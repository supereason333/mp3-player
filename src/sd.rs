use defmt::*;
use defmt_rtt as _;

use embedded_hal_bus::spi::NoDelay;
use heapless::Vec as HVec;

use embassy_futures::select::{Either, select};

use embassy_sync::blocking_mutex::raw::CriticalSectionRawMutex;
use embassy_sync::channel::Channel;
use embassy_sync::signal::Signal;

use embedded_sdmmc::{
    Directory, RawDirectory, RawFile, RawVolume, SdCard, ShortFileName, TimeSource, Timestamp,
    VolumeIdx, VolumeManager,
};

use embassy_rp::gpio::Output;
use embassy_rp::peripherals::SPI0;
use embassy_rp::spi;
use embassy_rp::spi::Async;
use embassy_rp::spi::Spi;

use embedded_hal_bus::spi::ExclusiveDevice;

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
pub const AUDIO_CHUNK_FRAMES: usize = AUDIO_CHUNK_BYTES / 4; // = 256

pub enum AudioSdRequest {
    Open(DirPath, ShortFileName),
    ReadChunk,
    Close,
}

pub enum AudioSdResponse {
    Opened { data_offset: u32, data_size: u32 }, // offset/size of the PCM data chunk, after header
    Chunk([u8; AUDIO_CHUNK_BYTES]),
    Eof,
    Error,
}

pub static AUDIO_SD_REQUEST: Channel<CriticalSectionRawMutex, AudioSdRequest, 2> = Channel::new();
pub static AUDIO_SD_RESPONSE: Signal<CriticalSectionRawMutex, AudioSdResponse> = Signal::new();

pub struct WavInfo {
    pub sample_rate: u32,
    pub bits_per_sample: u16,
    pub num_channels: u16,
}

pub struct AudioPlaybackState {
    volume: RawVolume,
    file: RawFile,
}

pub type DirListing = HVec<(ShortFileName, u32, bool), 32>;
pub type DirPath = HVec<ShortFileName, 16>;
type SdBlockDevice =
    SdCard<ExclusiveDevice<Spi<'static, SPI0, Async>, Output<'static>, Delay>, Delay>;
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
pub async fn sd_task(
    spi_device: ExclusiveDevice<Spi<'static, SPI0, Async>, Output<'static>, NoDelay>,
) -> ! {
    info!("[SD] SD task spawned");
    let sdcard = SdCard::new(spi_device, Delay);
    let sd_size = match sdcard.num_bytes() {
        Ok(size) => size,
        Err(e) => defmt::panic!(
            "Failed to get SD card size (Is card connected?) Error: {}",
            Debug2Format(&e)
        ),
    };
    info!("[SD] card size is {} bytes", sd_size);

    sdcard.spi(|spi_dev| {
        info!("[SD] reconfiguring SPI speed");
        spi_dev.bus_mut().set_config(&{
            let mut cfg = spi::Config::default();
            cfg.frequency = 24_000_000;
            cfg
        });
    });

    let mut volume_mgr = VolumeManager::new(sdcard, DummyTimesource::default());

    let mut playback: Option<AudioPlaybackState> = None;

    loop {
        match select(SD_REQUEST.receive(), AUDIO_SD_REQUEST.receive()).await {
            Either::First(ui_request) => match ui_request {
                SdRequest::ListDir(path) => {
                    let mut entries: DirListing = HVec::new();

                    let result = (|| -> Result<(), SdError> {
                        let volume0 = volume_mgr.open_volume(VolumeIdx(0))?;
                        let root = volume0.open_root_dir()?;
                        let result = list_dir_recursive(&root, &path, &mut entries);
                        root.close()?;
                        volume0.close()?;
                        result
                    })();

                    if let Err(e) = result {
                        error!("SD list error: {:?}", Debug2Format(&e));
                    }

                    SD_RESPONSE.signal(SdResponse::DirListing(entries));
                }
                SdRequest::ReadFile(path, name) => {
                    let mut buf: HVec<u8, 512> = HVec::new();

                    let result = (|| -> Result<(), SdError> {
                        let volume0 = volume_mgr.open_volume(VolumeIdx(0))?;
                        let root = volume0.open_root_dir()?;

                        let r = with_dir_at_path(&root, &path, |target_dir| {
                            let file = target_dir
                                .open_file_in_dir(name.clone(), embedded_sdmmc::Mode::ReadOnly)?;
                            buf.resize_default(512).ok();
                            let bytes_read = file.read(&mut buf)?;
                            buf.truncate(bytes_read);
                            file.close()?;
                            Ok(())
                        });

                        root.close()?;
                        volume0.close()?;
                        r
                    })();

                    if let Err(e) = result {
                        error!("[SD] ReadFile failed: {:?}", Debug2Format(&e));
                    }

                    SD_RESPONSE.signal(SdResponse::FileContents(buf));
                }
            },
            Either::Second(audio_request) => match audio_request {
                AudioSdRequest::Open(path, name) => {
                    let result = (|| -> Result<u32, SdError> {
                        let raw_volume = volume_mgr.open_raw_volume(VolumeIdx(0))?;
                        let raw_root = volume_mgr.open_root_dir(raw_volume)?;

                        let raw_dir = open_raw_dir_path(&mut volume_mgr, raw_root, &path)?; // raw-handle version of your recursive walker

                        let raw_file = volume_mgr.open_file_in_dir(
                            raw_dir,
                            name.clone(),
                            embedded_sdmmc::Mode::ReadOnly,
                        )?;

                        let mut header = [0u8; 44];
                        volume_mgr.read(raw_file, &mut header)?;
                        let data_size =
                            u32::from_le_bytes([header[40], header[41], header[42], header[43]]);

                        volume_mgr.close_dir(raw_dir)?; // done walking, don't need the dir handle anymore
                        // NOTE: raw_root and any intermediate dirs opened during the walk should be closed too

                        playback = Some(AudioPlaybackState {
                            volume: raw_volume,
                            file: raw_file,
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

                AudioSdRequest::ReadChunk => {
                    let start = Instant::now();
                    if let Some(state) = &playback {
                        let mut buf: [u8; AUDIO_CHUNK_BYTES] = [0u8; AUDIO_CHUNK_BYTES];
                        // buf.resize_default(AUDIO_CHUNK_BYTES).ok();
                        match volume_mgr.read(state.file, &mut buf) {
                            Ok(0) => AUDIO_SD_RESPONSE.signal(AudioSdResponse::Eof),
                            Ok(n) => {
                                // buf.truncate(n);
                                AUDIO_SD_RESPONSE.signal(AudioSdResponse::Chunk(buf));
                            }
                            Err(e) => {
                                error!("[SD] audio read failed: {:?}", Debug2Format(&e));
                                AUDIO_SD_RESPONSE.signal(AudioSdResponse::Error);
                            }
                        }
                    } else {
                        AUDIO_SD_RESPONSE.signal(AudioSdResponse::Error);
                    }
                    // info!("[SD] readchunk took {} ms", start.elapsed().as_millis())
                }

                AudioSdRequest::Close => {
                    if let Some(state) = playback.take() {
                        let _ = volume_mgr.close_file(state.file);
                        let _ = volume_mgr.close_volume(state.volume);
                    }
                    AUDIO_SD_RESPONSE.signal(AudioSdResponse::Eof);
                }
            },
        }
    }
}

fn with_dir_at_path<D, T, const DIRS: usize, const FILES: usize, const VOLS: usize, F, R>(
    dir: &Directory<D, T, DIRS, FILES, VOLS>,
    path: &[ShortFileName],
    f: F,
) -> Result<R, embedded_sdmmc::Error<D::Error>>
where
    D: embedded_sdmmc::BlockDevice,
    T: embedded_sdmmc::TimeSource,
    F: FnOnce(&Directory<D, T, DIRS, FILES, VOLS>) -> Result<R, embedded_sdmmc::Error<D::Error>>,
{
    match path.split_first() {
        None => f(dir), // reached target — run the caller's logic here
        Some((first, rest)) => {
            let child = dir.open_dir(first)?;
            let result = with_dir_at_path(&child, rest, f);
            child.close()?;
            result
        }
    }
}

fn list_dir_recursive<D, T, const DIRS: usize, const FILES: usize, const VOLS: usize>(
    dir: &Directory<D, T, DIRS, FILES, VOLS>,
    path: &[ShortFileName],
    entries: &mut DirListing,
) -> Result<(), embedded_sdmmc::Error<D::Error>>
where
    D: embedded_sdmmc::BlockDevice,
    T: embedded_sdmmc::TimeSource,
{
    with_dir_at_path(dir, path, |target_dir| {
        target_dir.iterate_dir(|entry| {
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
    })
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
    let mut current = start;
    for segment in path {
        let next = volume_mgr.open_dir(current, segment)?;
        if current != start {
            volume_mgr.close_dir(current)?; // dont close the caller's original `start` handle
        }
        current = next;
    }
    Ok(current)
}
