mod audio;
mod ui;

use audio::*;
use ui::*;

use defmt::*;
use defmt_rtt as _;

use embedded_hal_bus::spi::{ExclusiveDevice, NoDelay};
use esp_hal::time::Rate;

use embassy_sync::blocking_mutex::raw::CriticalSectionRawMutex;
use embassy_sync::channel::Channel;
use embassy_sync::signal::Signal;

use embedded_sdmmc::Directory;
use heapless::Vec as HVec;

use embassy_futures::select::{Either, select};

use embedded_sdmmc::{RawDirectory, SdCard, ShortFileName, TimeSource, Timestamp, VolumeManager};

use esp_hal::Async;
use esp_hal::gpio::Output;
use esp_hal::spi::Mode;
use esp_hal::spi::master::{Config, SpiDmaBus};

use embassy_time::Delay;

use static_cell::StaticCell;

pub type DirListing = HVec<(ShortFileName, u32, bool), 32>;
pub type DirPath = HVec<ShortFileName, 16>;

type SdBlockDevice =
    SdCard<ExclusiveDevice<SpiDmaBus<'static, Async>, Output<'static>, NoDelay>, Delay>;
type SdError = embedded_sdmmc::Error<<SdBlockDevice as embedded_sdmmc::BlockDevice>::Error>;

// UI stuff
// Simple requests
pub enum SdRequest {
    ListDir(DirPath),
    ReadFile(DirPath, ShortFileName), // whole-file read, capped size, for text/hex viewers
}

pub enum SdResponse {
    DirListing(DirListing),
    FileContents(HVec<u8, 512>),
}

// AUdio stuff
pub const AUDIO_CHUNK_BYTES: usize = 1024 * 8;
pub const AUDIO_CHUNK_FRAMES: usize = AUDIO_CHUNK_BYTES / 2;

// Static buffers for audio
pub static AUDIO_BUF_0: StaticCell<[u8; AUDIO_CHUNK_BYTES]> = StaticCell::new();
pub static AUDIO_BUF_1: StaticCell<[u8; AUDIO_CHUNK_BYTES]> = StaticCell::new();

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

pub static SD_REQUEST: Channel<CriticalSectionRawMutex, SdRequest, 4> = Channel::new();
pub static SD_RESPONSE: Signal<CriticalSectionRawMutex, SdResponse> = Signal::new();

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
    let mut volume_mgr = match set_up_sd(spi_device, cs) {
        Ok(card) => Some(VolumeManager::new(card, DummyTimesource::default())),
        Err(_e) => None,
    };

    let mut playback: Option<AudioPlaybackState> = None;

    // DOuble static buffer setup
    let buf0 = AUDIO_BUF_0.init([0u8; AUDIO_CHUNK_BYTES]);
    let buf1 = AUDIO_BUF_1.init([0u8; AUDIO_CHUNK_BYTES]);
    AUDIO_EMPTY.try_send(buf0).ok();
    AUDIO_EMPTY.try_send(buf1).ok();

    loop {
        match (
            &mut volume_mgr,
            select(AUDIO_SD_REQUEST.receive(), SD_REQUEST.receive()).await,
        ) {
            (Some(vm), Either::First(req)) => handle_audio_request(req, vm, &mut playback).await,
            (None, Either::First(req)) => empty_handle_audio(req).await,
            (Some(vm), Either::Second(req)) => handle_ui_request(req, vm).await,
            (None, Either::Second(req)) => empty_handle_ui(req).await,
        }
    }
}

fn set_up_sd(
    spi_device: SpiDmaBus<'static, Async>,
    cs: Output<'static>,
) -> Result<
    SdCard<ExclusiveDevice<SpiDmaBus<'static, Async>, Output<'static>, NoDelay>, Delay>,
    embedded_sdmmc::sdcard::Error,
> {
    let exclusive_device = match ExclusiveDevice::new_no_delay(spi_device, cs) {
        Ok(device) => device,
        Err(_e) => defmt::panic!("Failed to get exclusive device"),
    };
    let sdcard = SdCard::new(exclusive_device, Delay);

    let sd_size = sdcard.num_bytes()?;
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
    Ok(sdcard)
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
