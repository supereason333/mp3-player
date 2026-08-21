use defmt::*;
use defmt_rtt as _;

use embedded_hal_bus::spi::{ExclusiveDevice, NoDelay};
use esp_hal::time::Rate;
use static_cell::StaticCell;

use embassy_sync::pubsub::Error;
use embedded_sdmmc::Directory;
use heapless::Vec as HVec;

use embassy_futures::select::{Either, select};

use embassy_sync::blocking_mutex::raw::CriticalSectionRawMutex;
use embassy_sync::channel::Channel;
use embassy_sync::signal::Signal;

use embedded_sdmmc::{
    RawDirectory, RawFile, RawVolume, SdCard, ShortFileName, TimeSource, Timestamp, VolumeIdx,
    VolumeManager,
};

use esp_hal::Async;
use esp_hal::gpio::Output;
use esp_hal::spi::Mode;
use esp_hal::spi::master::{Config, Spi, SpiDmaBus};

use embassy_time::Delay;
use embassy_time::Instant;

// Simple requests
pub enum SdRequest {
    ListDir(DirPath),
    ReadFile(DirPath, ShortFileName), // whole-file read, capped size, for text/hex viewers
}

pub enum SdResponse {
    DirListing(DirListing),
    FileContents(HVec<u8, 512>),
}

pub static SD_REQUEST: Channel<CriticalSectionRawMutex, SdRequest, 4> = Channel::new();
pub static SD_RESPONSE: Signal<CriticalSectionRawMutex, SdResponse> = Signal::new();

// audio
pub const AUDIO_CHUNK_BYTES: usize = 1024 * 8;
pub const AUDIO_CHUNK_FRAMES: usize = AUDIO_CHUNK_BYTES / 2;

// Static buffers for audio
static AUDIO_BUF_0: StaticCell<[u8; AUDIO_CHUNK_BYTES]> = StaticCell::new();
static AUDIO_BUF_1: StaticCell<[u8; AUDIO_CHUNK_BYTES]> = StaticCell::new();

pub static AUDIO_FILLED: Channel<CriticalSectionRawMutex, &'static mut [u8; AUDIO_CHUNK_BYTES], 2> =
    Channel::new();
pub static AUDIO_EMPTY: Channel<CriticalSectionRawMutex, &'static mut [u8; AUDIO_CHUNK_BYTES], 2> =
    Channel::new();

pub enum AudioSdRequest {
    Open(DirPath, ShortFileName),
    ReadChunk,
    Close,
}

pub enum AudioSdResponse {
    Opened { data_offset: u32, data_size: u32 }, // offset/size of the PCM data chunk, after header
    Eof,
    Error,
    Closed,
}

pub static AUDIO_SD_REQUEST: Channel<CriticalSectionRawMutex, AudioSdRequest, 2> = Channel::new();
pub static AUDIO_SD_RESPONSE: Signal<CriticalSectionRawMutex, AudioSdResponse> = Signal::new();

pub struct AudioPlaybackState {
    volume: RawVolume,
    file: RawFile,
}

pub type DirListing = HVec<(ShortFileName, u32, bool), 32>;
pub type DirPath = HVec<ShortFileName, 16>;
// type SdBlockDevice =
//     SdCard<ExclusiveDevice<Spi<'static, SPI0, Async>, Output<'static>, Delay>, Delay>;
// type SdError = embedded_sdmmc::Error<<SdBlockDevice as embedded_sdmmc::BlockDevice>::Error>;
type SdBlockDevice =
    SdCard<ExclusiveDevice<SpiDmaBus<'static, Async>, Output<'static>, NoDelay>, Delay>;
type SdError = embedded_sdmmc::Error<<SdBlockDevice as embedded_sdmmc::BlockDevice>::Error>;

/// Code from https://github.com/rp-rs/rp-hal-boards/blob/main/boards/rp-pico/examples/pico_spi_sd_card.rs
/// A dummy timesource, which is mostly important for creating files.
#[derive(Default)]
pub struct DummyTimesource();

impl TimeSource for DummyTimesource {
    // In theory you could use the RTC of the rp2040 here, if you had
    // any external time synchronizing device.
    fn get_timestamp(&self) -> Timestamp {
        Timestamp {
            year_since_1970: 0,
            zero_indexed_month: 0,
            zero_indexed_day: 0,
            hours: 0,
            minutes: 0,
            seconds: 0,
        }
    }
}

#[embassy_executor::task]
pub async fn sd_task(spi_device: SpiDmaBus<'static, Async>, cs: Output<'static>) -> ! {
    info!("[SD] SD task spawned");
    let exclusive_device = match ExclusiveDevice::new_no_delay(spi_device, cs) {
        Ok(device) => device,
        Err(_e) => defmt::panic!("Failed to get exclusive device"),
    };
    let sdcard = SdCard::new(exclusive_device, Delay);

    let sd_size = match sdcard.num_bytes() {
        Ok(size) => size,
        Err(e) => defmt::panic!(
            "Failed to get SD card size (Is card connected?) Error: {}",
            Debug2Format(&e)
        ),
    };
    info!("[SD] card size is {} bytes", sd_size);

    sdcard
        .spi(|spi_dev| {
            info!("[SD] reconfiguring SPI speed");
            spi_dev.bus_mut().apply_config(
                &Config::default()
                    .with_frequency(Rate::from_mhz(24))
                    .with_mode(Mode::_0),
            )
        })
        .unwrap();

    let volume_mgr = VolumeManager::new(sdcard, DummyTimesource::default());

    let mut playback: Option<AudioPlaybackState> = None;

    // DOuble static buffer setup
    let buf0 = AUDIO_BUF_0.init([0u8; AUDIO_CHUNK_BYTES]);
    let buf1 = AUDIO_BUF_1.init([0u8; AUDIO_CHUNK_BYTES]);
    AUDIO_EMPTY.try_send(buf0).ok();
    AUDIO_EMPTY.try_send(buf1).ok();

    loop {
        if let Ok(req) = AUDIO_SD_REQUEST.try_receive() {
            handle_audio_request(req, &volume_mgr, &mut playback).await;
            continue;
        }

        match select(AUDIO_SD_REQUEST.receive(), SD_REQUEST.receive()).await {
            Either::First(request) => {
                handle_audio_request(request, &volume_mgr, &mut playback).await
            }
            Either::Second(request) => handle_ui_request(request, &volume_mgr).await,
        }
    }
}

async fn handle_ui_request<'a, D, T, const DIRS: usize, const FILES: usize, const VOLS: usize>(
    request: SdRequest,
    volume_mgr: &'a VolumeManager<D, T, DIRS, FILES, VOLS>,
) where
    D: embedded_sdmmc::BlockDevice,
    T: embedded_sdmmc::TimeSource,
{
    match request {
        SdRequest::ListDir(path) => {
            let mut entries: HVec<(ShortFileName, u32, bool), 32> = HVec::new();
            let volume = volume_mgr.open_volume(VolumeIdx(0)).unwrap();
            let root = volume.open_root_dir().unwrap();
            let directory = open_dir_path(volume_mgr, root, &path).unwrap();

            directory
                .iterate_dir(|entry| {
                    if entry.attributes.is_system()
                        || entry.attributes.is_volume()
                        || entry.name.base_name()[0] == b'_'
                        || (entry.name.base_name().len() == 1 && entry.name.base_name()[0] == b'.')
                    {
                        return;
                    }

                    let _ = entries.push((
                        entry.name.clone(),
                        entry.size,
                        entry.attributes.is_directory(),
                    ));
                })
                .unwrap();

            directory.close().unwrap();
            volume.close().unwrap();

            SD_RESPONSE.signal(SdResponse::DirListing(entries));
        }
        SdRequest::ReadFile(path, name) => {
            let mut buf: HVec<u8, 512> = HVec::new();
            // let buf = [0u8; 512];

            // TEMPORARYLY NOT USED REMOVED, WRITE AGAIN LATER!
            // Just retunrs empty data
            //
            // Use static buffer maybe?

            SD_RESPONSE.signal(SdResponse::FileContents(buf));
        }
    }
}

async fn handle_audio_request<'a, D, T, const DIRS: usize, const FILES: usize, const VOLS: usize>(
    request: AudioSdRequest,
    volume_mgr: &'a VolumeManager<D, T, DIRS, FILES, VOLS>,
    playback_state: &mut Option<AudioPlaybackState>,
) where
    D: embedded_sdmmc::BlockDevice,
    T: embedded_sdmmc::TimeSource,
{
    match request {
        AudioSdRequest::Open(path, name) => {
            let result = (|| -> Result<u32, SdError> {
                let volume = volume_mgr.open_raw_volume(VolumeIdx(0)).unwrap();
                let root = volume_mgr.open_root_dir(volume).unwrap(); // TODO: HAndle these errors!

                let volume = volume.to_volume(volume_mgr);
                let root = root.to_directory(volume_mgr);

                // This is the new working directory
                let dir = open_dir_path(&volume_mgr, root, &path).unwrap();

                let file = dir
                    .open_file_in_dir(name.clone(), embedded_sdmmc::Mode::ReadOnly)
                    .unwrap();

                let mut header = [0u8; 44];
                if file.read(&mut header).unwrap() != 44 {
                    info!("[SD] File header less than 44 bytes");
                    return Err(embedded_sdmmc::Error::EndOfFile);
                }

                let data_size =
                    u32::from_le_bytes([header[40], header[41], header[42], header[43]]);

                // Clear it before if it was accdently left unclosed
                if let Some(mut state) = playback_state.take() {
                    match close(&mut state, volume_mgr) {
                        Ok(()) => {}                                // yay
                        Err(embedded_sdmmc::Error::BadHandle) => {} // Prob already closed
                        Err(e) => {} // Otehr error i prob dont care about
                    }
                    // it should be dropped even if it errors because i said so
                }

                *playback_state = Some(AudioPlaybackState {
                    volume: volume.to_raw_volume(),
                    file: file.to_raw_file(),
                });
                Ok(data_size)
            })();

            match result {
                Ok(size) => AUDIO_SD_RESPONSE.signal(AudioSdResponse::Opened {
                    data_offset: 44,
                    data_size: size,
                }),
                Err(e) => {
                    error!("[SD] audio open failed: {:?}", Debug2Format(&e));
                    AUDIO_SD_RESPONSE.signal(AudioSdResponse::Error);
                }
            }
        }
        AudioSdRequest::Close => {
            if let Some(mut state) = playback_state.take() {
                if let Err(e) = close(&mut state, volume_mgr) {
                    // Error
                    // TODO: Do something useful, propogate back to caller with signal?
                    // When like I write wrapper module with function wrappers for these signals
                }
            }
            AUDIO_SD_RESPONSE.signal(AudioSdResponse::Closed);
        }
        AudioSdRequest::ReadChunk => {
            // Make sure buf is NOT DROPPED
            // info!("[SD] ReadChunk Recieved");
            let buf = AUDIO_EMPTY.receive().await; // Wait for a free buffer
            if let Some(state) = &playback_state {
                match volume_mgr.read(state.file, buf.as_mut_slice()) {
                    Ok(0) => {
                        // Dosent close it, need seperate close on EOF
                        // let mut state = playback_state.take().unwrap();
                        // close(&mut state, volume_mgr).unwrap();
                        AUDIO_EMPTY.send(buf).await;
                        AUDIO_SD_RESPONSE.signal(AudioSdResponse::Eof);
                    }
                    Ok(n) => {
                        // Successful read
                        if n < AUDIO_CHUNK_BYTES {
                            buf[n..].fill(0);
                            info!("Trailing zeros in audio chunk, len {}", n);
                        }
                        AUDIO_FILLED.send(buf).await; // Hand off data to consumer
                    }
                    Err(e) => {
                        AUDIO_EMPTY.send(buf).await;
                        AUDIO_SD_RESPONSE.signal(AudioSdResponse::Error);
                    }
                }
            } else {
                // No state
                AUDIO_EMPTY.send(buf).await; // put back onto empty stack
                AUDIO_SD_RESPONSE.signal(AudioSdResponse::Error);
            }
        }
    }
}

fn close<'a, D, T, const DIRS: usize, const FILES: usize, const VOLS: usize>(
    state: &mut AudioPlaybackState,
    volume_mgr: &'a VolumeManager<D, T, DIRS, FILES, VOLS>,
) -> Result<(), embedded_sdmmc::Error<D::Error>>
where
    D: embedded_sdmmc::BlockDevice,
    T: embedded_sdmmc::TimeSource,
{
    let file_result = volume_mgr.close_file(state.file);
    let volume_result = volume_mgr.close_volume(state.volume);

    if let Err(e) = &file_result {
        error!("[SD] failed to close file: {:?}", Debug2Format(e));
    }
    if let Err(e) = &volume_result {
        error!("[SD] failed to close volume: {:?}", Debug2Format(e));
    }

    // surface the first error to the caller, if either failed
    volume_result?;
    file_result?;
    Ok(())
}

fn open_dir_path<'a, D, T, const DIRS: usize, const FILES: usize, const VOLS: usize>(
    volume_mgr: &'a VolumeManager<D, T, DIRS, FILES, VOLS>,
    start: Directory<'a, D, T, DIRS, FILES, VOLS>,
    path: &[ShortFileName],
) -> Result<Directory<'a, D, T, DIRS, FILES, VOLS>, embedded_sdmmc::Error<D::Error>>
where
    D: embedded_sdmmc::BlockDevice,
    T: embedded_sdmmc::TimeSource,
{
    if path.is_empty() {
        return Ok(start);
    }

    // drop to raw immediately: releases the borrow-holding wrapper before recursion,
    // start is not closed here since to_raw_directory() releases rather than closes
    let raw_start = start.to_raw_directory();

    let result = open_raw_dir_path(volume_mgr, raw_start, path);

    match result {
        Ok(raw_final) => Ok(raw_final.to_directory(volume_mgr)),
        Err(e) => {
            // raw_start (and all intermediates) already closed by open_raw_dir_path's
            // internal guards on the failure path -- nothing left to clean up here
            Err(e)
        }
    }
}

fn open_raw_dir_path<D, T, const DIRS: usize, const FILES: usize, const VOLS: usize>(
    volume_mgr: &VolumeManager<D, T, DIRS, FILES, VOLS>,
    start: RawDirectory,
    path: &[ShortFileName],
) -> Result<RawDirectory, embedded_sdmmc::Error<D::Error>>
where
    D: embedded_sdmmc::BlockDevice,
    T: embedded_sdmmc::TimeSource,
{
    if path.is_empty() {
        return Ok(start);
    }

    let mut current = start;
    let mut current_guard: Option<Directory<'_, D, T, DIRS, FILES, VOLS>> = None;

    for segment in path {
        // if this fails, `current_guard` (the previous intermediate, if any) drops here
        // automatically and closes itself. `start` is untouched since it's never guarded.
        let next_raw = volume_mgr.open_dir(current, segment)?;
        let next_guard = next_raw.to_directory(volume_mgr);

        // close the previous intermediate now that we've moved past it
        if let Some(dir) = current_guard.take() {
            dir.close().unwrap();
        }

        volume_mgr.close_dir(current).unwrap();
        current = next_raw;
        current_guard = Some(next_guard);
    }

    // success: extract the raw handle from the final guard instead of letting it close
    Ok(current_guard.unwrap().to_raw_directory())
}
