// sd/mod.rs
pub mod audio;
pub mod client;
pub mod mp3parse;
mod ui;
pub mod wavparse;

use audio::*;
use embedded_sdmmc::DirEntry;
use heapless::spsc::Producer;
use ui::*;

use defmt::*;
use defmt_rtt as _;

use core::sync::atomic::AtomicBool;
use core::sync::atomic::AtomicU8;
use core::sync::atomic::Ordering;
use embedded_hal_bus::spi::{ExclusiveDevice, NoDelay};
use esp_hal::time::Rate;

use embassy_sync::blocking_mutex::raw::CriticalSectionRawMutex;
use embassy_sync::channel::Channel;
use embassy_sync::mutex::Mutex;
use embassy_sync::signal::Signal;
use embassy_sync::watch::Watch;

use embedded_sdmmc::Directory;
use heapless::Vec as HVec;

use embassy_futures::select::{Either, Either3, select, select3};

use embedded_sdmmc::{RawDirectory, SdCard, ShortFileName, TimeSource, Timestamp, VolumeManager};

use esp_hal::Async;
use esp_hal::gpio::Output;
use esp_hal::spi::Mode;
use esp_hal::spi::master::{Config, SpiDmaBus};

use embassy_time::Delay;
use embassy_time::{Duration, Timer};

use static_cell::StaticCell;

use crate::sd::mp3parse::Id3v2Tag;

pub type DirPath = HVec<ShortFileName, 16>;
pub type AudioChunk = &'static mut [u8; AUDIO_CHUNK_BYTES];

type SdBlockDevice =
    SdCard<ExclusiveDevice<SpiDmaBus<'static, Async>, Output<'static>, NoDelay>, Delay>;
type SdError = embedded_sdmmc::Error<<SdBlockDevice as embedded_sdmmc::BlockDevice>::Error>;

// UI stuff
// Simple requests
enum SdRequest {
    ListDir(DirPath, usize),
    ReadFile(DirPath, ShortFileName), // whole-file read, capped size, for text/hex viewers
}

enum SdResponse {
    /// When the contents of DIRECTORY_LIST is ready, bool is more after
    DirLoaded(bool),
    FileContents(HVec<u8, 512>),
    SetupFinished(Result<(), ()>),
}

/// Stores the directory list which is queried
pub static DIRECTORY_LIST: Mutex<CriticalSectionRawMutex, HVec<DirEntry, 16>> =
    Mutex::new(HVec::new());

// AUdio stuff
pub const AUDIO_CHUNK_BYTES: usize = 1024 * 4;
pub const AUDIO_CHUNK_FRAMES: usize = AUDIO_CHUNK_BYTES / 2;

// Static buffers for audio
static AUDIO_BUF_0: StaticCell<[u8; AUDIO_CHUNK_BYTES]> = StaticCell::new();
static AUDIO_BUF_1: StaticCell<[u8; AUDIO_CHUNK_BYTES]> = StaticCell::new();
static AUDIO_BUF_2: StaticCell<[u8; AUDIO_CHUNK_BYTES]> = StaticCell::new();
static AUDIO_BUF_3: StaticCell<[u8; AUDIO_CHUNK_BYTES]> = StaticCell::new();

static AUDIO_FILLED: Channel<CriticalSectionRawMutex, AudioChunk, 4> = Channel::new();
static AUDIO_EMPTY: Channel<CriticalSectionRawMutex, AudioChunk, 4> = Channel::new();

enum AudioSdRequest {
    Open(DirPath, ShortFileName),
    Close,
}

enum AudioSdResponse {
    Opened,
    Error,
    Closed,
}

static AUDIO_SD_REQUEST: Channel<CriticalSectionRawMutex, AudioSdRequest, 2> = Channel::new();
static AUDIO_SD_RESPONSE: Signal<CriticalSectionRawMutex, AudioSdResponse> = Signal::new();

static SD_REQUEST: Channel<CriticalSectionRawMutex, SdRequest, 4> = Channel::new();
static SD_RESPONSE: Signal<CriticalSectionRawMutex, SdResponse> = Signal::new();

/// If track is loaded and ready to be played
pub static TRACK_LOADED: AtomicBool = AtomicBool::new(false);
/// UI metadata like name and artist
pub static TRACK_METADATA: Watch<CriticalSectionRawMutex, Option<TrackMetadata>, 2> = Watch::new();
/// Playback importnat data like sample rate
pub static TRACK_AUDIO_INFO: Mutex<CriticalSectionRawMutex, Option<AudioInfo>> = Mutex::new(None);
/// The loaded file's format
pub static TRACK_FORMAT: AtomicU8 = AtomicU8::new(AudioType::MP3 as u8);

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
    spi_device: SpiDmaBus<'static, Async>,
    cs: Output<'static>,
    mut ring_producer: Producer<'static, u8>,
) -> ! {
    info!("[SD] SD task spawned");

    let volume_mgr = match set_up_sd(spi_device, cs).await {
        Ok(card) => {
            SD_RESPONSE.signal(SdResponse::SetupFinished(Ok(())));
            info!("[SD] Got volume mgr");
            VolumeManager::new(card, DummyTimesource::default())
        }
        Err(_e) => {
            SD_RESPONSE.signal(SdResponse::SetupFinished(Err(())));
            loop {
                Timer::after(Duration::from_secs(100)).await;
            }
        }
    };

    // let mut volume_mgr: Option<
    //     VolumeManager<
    //         SdCard<ExclusiveDevice<SpiDmaBus<'static, Async>, Output<'static>, NoDelay>, Delay>,
    //         DummyTimesource,
    //     >,
    // > = None; // NO SD CARD OVERRIDE, REMOVE WHEN CARD INSERTED
    // warn!("[SD] No card override enabled!");

    let mut playback: Option<AudioPlaybackState> = None;
    TRACK_LOADED.store(false, Ordering::Relaxed);

    info!("[SD] Setting up static buffers");
    // static buffer setup
    let buf0 = AUDIO_BUF_0.init([0u8; AUDIO_CHUNK_BYTES]);
    let buf1 = AUDIO_BUF_1.init([0u8; AUDIO_CHUNK_BYTES]);
    let buf2 = AUDIO_BUF_2.init([0u8; AUDIO_CHUNK_BYTES]);
    let buf3 = AUDIO_BUF_3.init([0u8; AUDIO_CHUNK_BYTES]);
    AUDIO_EMPTY.try_send(buf0).ok();
    AUDIO_EMPTY.try_send(buf1).ok();
    AUDIO_EMPTY.try_send(buf2).ok();
    AUDIO_EMPTY.try_send(buf3).ok();

    info!("[SD] Finished setting up");
    loop {
        // info!("TRACK_LOADED    : {}", TRACK_LOADED.load(Ordering::Relaxed));
        // info!("playback is some: {}", playback.is_some());

        match (
            &playback,
            TRACK_LOADED.load(Ordering::Relaxed),
            AudioType::from_u8(TRACK_FORMAT.load(Ordering::Relaxed)),
        ) {
            // Playing WAV, fill PCM into buffer directly
            (Some(state), true, AudioType::WAV) => match select3(
                AUDIO_SD_REQUEST.receive(),
                SD_REQUEST.receive(),
                AUDIO_EMPTY.ready_to_receive(),
            )
            .await
            {
                Either3::First(req) => handle_audio_request(req, &volume_mgr, &mut playback).await,
                Either3::Second(req) => handle_ui_request(req, &volume_mgr).await,
                Either3::Third(()) => {
                    // info!("[SD] Filling chunk");
                    let mut eof = false;
                    while let Ok(buf) = AUDIO_EMPTY.try_receive() {
                        match volume_mgr.read(state.file, buf.as_mut_slice()) {
                            Ok(0) => {
                                AUDIO_EMPTY.send(buf).await;
                                eof = true;
                            }
                            Ok(n) => {
                                // Successful read
                                if n < AUDIO_CHUNK_BYTES {
                                    buf[n..].fill(0);
                                    info!("Trailing zeros in audio chunk, len {}", n);
                                    eof = true;
                                }
                                AUDIO_FILLED.send(buf).await; // Hand off data to consumer
                            }
                            Err(_e) => {
                                AUDIO_EMPTY.send(buf).await;
                                eof = true;
                            }
                        }
                    }
                    if eof {
                        if let Some(mut state) = playback.take() {
                            if let Err(e) = close(&mut state, &volume_mgr) {
                                error!("[SD] ERR closing file on EOF {}", Debug2Format(&e));
                            }
                        }

                        playback = None;
                        TRACK_LOADED.store(false, Ordering::Relaxed);
                        let sender = TRACK_METADATA.sender();
                        sender.send(None);
                    }
                }
            },
            // Playing MP3, feed the decoder ring buffer
            (Some(state), true, AudioType::MP3) => match select3(
                AUDIO_SD_REQUEST.receive(),
                SD_REQUEST.receive(),
                Timer::after(Duration::from_millis(13)),
                // 1152 samples per frame to poll the ring buffer
                // / 44100 hz * 1000
                // = 26 ms
                // Half that for some headroom = 13 ms
            )
            .await
            {
                Either3::First(req) => handle_audio_request(req, &volume_mgr, &mut playback).await,
                Either3::Second(req) => handle_ui_request(req, &volume_mgr).await,
                Either3::Third(()) => {
                    let mut eof = false;
                    let free = ring_producer.capacity() - ring_producer.len();
                    let chunks = free / 256;
                    'outer: for _ in 0..chunks {
                        let mut buf = [0u8; 256];
                        match volume_mgr.read(state.file, &mut buf) {
                            Ok(0) => eof = true,
                            Ok(n) => {
                                if n < 256 {
                                    eof = true;
                                }
                                for b in 0..n {
                                    if ring_producer.enqueue(buf[b]).is_err() {
                                        break 'outer;
                                    }
                                }
                            }
                            Err(_e) => eof = true,
                        }
                        if eof {
                            if let Some(mut state) = playback.take() {
                                if let Err(e) = close(&mut state, &volume_mgr) {
                                    error!("[SD] ERR closing file on EOF {}", Debug2Format(&e));
                                }
                            }

                            playback = None;
                            TRACK_LOADED.store(false, Ordering::Relaxed);
                            let sender = TRACK_METADATA.sender();
                            sender.send(None);
                            break;
                        }
                    }
                }
            },
            // Else, just watch for requests
            _ => match select(AUDIO_SD_REQUEST.receive(), SD_REQUEST.receive()).await {
                Either::First(req) => handle_audio_request(req, &volume_mgr, &mut playback).await,
                Either::Second(req) => handle_ui_request(req, &volume_mgr).await,
            },
        }
    }
}

async fn set_up_sd(
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

    info!("[SD] Getting Card size");
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
    info!("Finished reconfiguring speed");
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
        let next_raw = volume_mgr.open_dir(current, segment)?;
        let next_guard = next_raw.to_directory(volume_mgr);

        match current_guard.take() {
            Some(dir) => {
                // `dir` wraps the same handle as `current` — closing it
                // here is sufficient, no separate close_dir(current) needed.
                dir.close().unwrap();
            }
            None => {
                // First iteration only: `current` (== start) has no guard,
                // so it needs an explicit close of its own.
                volume_mgr.close_dir(current).unwrap();
            }
        }

        current = next_raw;
        current_guard = Some(next_guard);
    }

    Ok(current_guard.unwrap().to_raw_directory())
}
