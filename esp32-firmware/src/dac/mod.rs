// dac/mod.rs
pub mod client;

use core::sync::atomic::{AtomicBool, Ordering};
use defmt::*;
use defmt_rtt as _;
use embassy_futures::select::Either;
use embassy_futures::select::select;
use esp_hal::i2s::master::Channels;
use esp_hal::i2s::master::UnitConfig;
use esp_hal::time::Rate;
use heapless::Deque;
use static_cell::StaticCell;

use embassy_sync::blocking_mutex::raw::CriticalSectionRawMutex;
use embassy_sync::signal::Signal;
use embassy_time::{Duration, Timer};

use embedded_sdmmc::ShortFileName;

use esp_hal::Async;
use esp_hal::i2s::master::I2sTx;
use esp_hal::i2s::master::{Config as I2sConfig, DataFormat, I2s};

use crate::dac::DacRequest::Start;
use crate::sd;
use crate::sd::DirPath;
use crate::sd::TRACK_AUDIO_INFO;
use crate::sd::client::*;

static DAC_REQUEST: Signal<CriticalSectionRawMutex, DacRequest> = Signal::new();
static DAC_RESPONSE: Signal<CriticalSectionRawMutex, DacResponse> = Signal::new();

/// True whenever a track is open (playing or paused). Mirrors current.is_some().
static DAC_LOADED: AtomicBool = AtomicBool::new(false);
/// True only while genuinely paused (i.e. inside wait_paused()).
static DAC_PAUSED: AtomicBool = AtomicBool::new(false);

enum DacRequest {
    Start(DirPath, ShortFileName),
    StartQueue,
    QueueAdd(DirPath, ShortFileName),
    Play,
    Pause,
    Stop,
    GetTrackInfo,
    GetQueue,
    ClearQueue,
    GetStatus,
    Skip,
}

enum DacResponse {
    Opened,
    Closed,
    Paused,
    Playing,
    Error(DacError),
    TrackInfo(Result<(DirPath, ShortFileName, i32), i32>),
    Status { loaded: bool, paused: bool },
    AddedToQueue,
}

#[derive(defmt::Format, Debug)]
pub enum DacError {
    NoAudioLoaded,
    CantOpenFile,
    AlreadyPlaying,
    UnknownResponse,
    Unimplimented,
    QueueEmpty,
    QueueFull,
}

pub const SAMPLE_RATE: u32 = 48_000;
pub const BIT_DEPTH: u32 = 16;

const DMA_BUFFER_SIZE: usize = 4 * 4092;

const I2S_CHUNK_BYTES: usize = crate::sd::AUDIO_CHUNK_FRAMES * 4;

static I2S_SCRATCH: StaticCell<[u8; I2S_CHUNK_BYTES]> = StaticCell::new();

#[embassy_executor::task]
pub async fn dac_task(
    i2s_tx: I2sTx<'static, Async>,
    tx_buffer: &'static mut [u8; DMA_BUFFER_SIZE],
) {
    info!("[DAC] DAC task spawned");

    DAC_LOADED.store(false, Ordering::Relaxed);
    DAC_PAUSED.store(false, Ordering::Relaxed);

    let mut player = Player::new(i2s_tx, tx_buffer);

    loop {
        player.stopped().await;
        player.playing().await;
    }
}

struct Player {
    tracknumber: i32,
    queue: Deque<(DirPath, ShortFileName), 16>,
    tx_buffer: &'static mut [u8; DMA_BUFFER_SIZE],
    i2s: I2sTx<'static, Async>,
    current: Option<(DirPath, ShortFileName)>,
}

/// Why play_loaded() ended, decided by playing().
enum PlayOutcome {
    Stop,
    Next,
    SwitchTo(DirPath, ShortFileName),
}

/// Why the paused sub-loop ended, decided by play_loaded().
enum PauseOutcome {
    Resume,
    Stop,
    SwitchTo(DirPath, ShortFileName),
    Next,
}

impl Player {
    fn new(i2s: I2sTx<'static, Async>, tx_buffer: &'static mut [u8; DMA_BUFFER_SIZE]) -> Self {
        Self {
            tracknumber: 0,
            queue: Deque::new(),
            tx_buffer,
            i2s,
            current: None,
        }
    }

    /// Waiting for something to play. Returns once `self.current` is Some.
    async fn stopped(&mut self) {
        self.current = None;
        DAC_LOADED.store(false, Ordering::Relaxed);
        DAC_PAUSED.store(false, Ordering::Relaxed);
        audio_close().await.ok();
        loop {
            match DAC_REQUEST.wait().await {
                DacRequest::Start(path, name) => {
                    self.current = Some((path, name));
                    DAC_LOADED.store(true, Ordering::Relaxed);
                    return;
                }
                DacRequest::StartQueue => {
                    let (path, name) = match self.queue.pop_front() {
                        Some((path, name)) => (path, name),
                        None => {
                            info!("[DAC] Start queue but queue empty");
                            DAC_RESPONSE.signal(DacResponse::Error(DacError::QueueEmpty));
                            continue;
                        }
                    };
                    self.current = Some((path, name));
                    DAC_LOADED.store(true, Ordering::Relaxed);
                    return;
                }
                DacRequest::QueueAdd(path, name) => match self.queue.push_back((path, name)) {
                    Ok(()) => DAC_RESPONSE.signal(DacResponse::AddedToQueue),
                    Err(_n) => DAC_RESPONSE.signal(DacResponse::Error(DacError::QueueFull)),
                },
                DacRequest::GetTrackInfo => {
                    DAC_RESPONSE.signal(DacResponse::TrackInfo(Err(self.tracknumber)));
                }
                DacRequest::GetStatus => {
                    DAC_RESPONSE.signal(DacResponse::Status {
                        loaded: false,
                        paused: false,
                    });
                }
                DacRequest::ClearQueue => {
                    self.queue.clear();
                }
                DacRequest::GetQueue => {
                    // TODO: no DacResponse variant carries queue contents yet.
                }
                DacRequest::Play | DacRequest::Pause | DacRequest::Stop | DacRequest::Skip => {
                    DAC_RESPONSE.signal(DacResponse::Error(DacError::NoAudioLoaded));
                }
            }
        }
    }

    /// Has a track loaded (playing or paused). Returns once `self.current` is None.
    async fn playing(&mut self) {
        loop {
            let Some((path, name)) = self.current.clone() else {
                DAC_RESPONSE.signal(DacResponse::Error(DacError::NoAudioLoaded));
                DAC_LOADED.store(false, Ordering::Relaxed);
                info!("[DAC] playing but current is none");
                return;
            };

            if audio_open(path.clone(), name.clone()).await.is_err() {
                DAC_RESPONSE.signal(DacResponse::Error(DacError::CantOpenFile));
                info!("[DAC] SD audio open error");
                return;
            }
            DAC_RESPONSE.signal(DacResponse::Opened);
            self.tracknumber += 1;
            info!("[DAC] Track number {}", self.tracknumber);
            match self.play_loaded().await {
                PlayOutcome::Stop => {
                    audio_close().await.ok();
                    DAC_RESPONSE.signal(DacResponse::Closed);
                    info!("[DAC] Play outcome: Stop");
                    return;
                }
                PlayOutcome::Next => {
                    audio_close().await.ok();
                    info!("[DAC] Play outcome: Next");
                    match self.queue.pop_front() {
                        Some((p, n)) => self.current = Some((p, n)),
                        None => {
                            info!("[DAC] Empty queue, stopping");
                            return;
                        }
                    }
                }
                PlayOutcome::SwitchTo(p, n) => {
                    info!("[DAC] Play outcome: Switch to");
                    audio_close().await.ok();
                    self.current = Some((p, n));
                }
            }
        }
    }

    async fn play_loaded(&mut self) -> PlayOutcome {
        info!("[DAC] Playing loaded");

        // Because parsers kinda shit atm, not gonna use header data for the config
        // Just gonna assume sample rate and stuff, make sure to input correctly formatted data
        // let i2s_config: UnitConfig;

        // let guard = TRACK_AUDIO_INFO.lock().await;
        // if let Some(info) = &*guard {
        //     match info {
        //         sd::audio::AudioInfo::MP3(data) => {
        //             if data.stereo {
        //                 i2s_config = UnitConfig::new_tdm_philips()
        //                     .with_channels(Channels::STEREO)
        //                     .with_sample_rate(Rate::from_hz(data.sample_rate))
        //                     .with_data_format(DataFormat::Data16Channel16);
        //             } else {
        //                 i2s_config = UnitConfig::new_tdm_philips()
        //                     .with_channels(Channels::MONO)
        //                     .with_sample_rate(Rate::from_hz(data.sample_rate))
        //                     .with_data_format(DataFormat::Data16Channel16);
        //             }
        //         }
        //         sd::audio::AudioInfo::WAV(data) => match data.channels {
        //             1 => {
        //                 info!("1Sample rate: {}", data.sample_rate);
        //                 i2s_config = UnitConfig::new_tdm_philips()
        //                     .with_channels(Channels::MONO)
        //                     .with_sample_rate(Rate::from_hz(data.sample_rate))
        //                     .with_data_format(DataFormat::Data16Channel16)
        //             }
        //             2 => {
        //                 info!("2Sample rate: {}", data.sample_rate);
        //                 i2s_config = UnitConfig::new_tdm_philips()
        //                     .with_channels(Channels::STEREO)
        //                     .with_sample_rate(Rate::from_hz(data.sample_rate))
        //                     .with_data_format(DataFormat::Data16Channel16);
        //             }
        //             _ => {
        //                 info!("{}Sample rate: {}", data.channels, data.sample_rate);
        //                 i2s_config = UnitConfig::new_tdm_philips()
        //                     .with_channels(Channels::STEREO)
        //                     .with_sample_rate(Rate::from_hz(data.sample_rate))
        //                     .with_data_format(DataFormat::Data16Channel16);
        //             }
        //         },
        //     }
        // } else {
        //     i2s_config = UnitConfig::new_tdm_philips()
        //         .with_channels(Channels::STEREO)
        //         .with_sample_rate(Rate::from_khz(48))
        //         .with_data_format(DataFormat::Data16Channel16);
        // }

        // if let Err(e) = self.i2s.apply_config(&i2s_config) {
        //     error!("[DAC] I2S apply config error {}", Debug2Format(&e));
        //     // Apply some default value instead
        //     self.i2s
        //         .apply_config(
        //             &UnitConfig::new_tdm_philips()
        //                 .with_channels(Channels::STEREO)
        //                 .with_sample_rate(Rate::from_khz(48))
        //                 .with_data_format(DataFormat::Data16Channel16),
        //         )
        //         .ok();
        // }

        let mut transfer = self
            .i2s
            .write_dma_circular(self.tx_buffer)
            .expect("failed to start I2S DMA transfer");

        loop {
            if DAC_PAUSED.load(Ordering::Relaxed) {
                core::mem::drop(transfer);
                match self.wait_paused().await {
                    PauseOutcome::Resume => {
                        DAC_PAUSED.store(false, Ordering::Relaxed);
                        transfer = self
                            .i2s
                            .write_dma_circular(self.tx_buffer)
                            .expect("failed to restart I2S DMA transfer");
                    }
                    PauseOutcome::Stop => return PlayOutcome::Stop,
                    PauseOutcome::SwitchTo(p, n) => return PlayOutcome::SwitchTo(p, n),
                    PauseOutcome::Next => return PlayOutcome::Next,
                }
            }

            // Sleep time should be either on frame drain time for eash 8k audio chunk
            // or a little less for a bit of a leaway to catch up or someting
            // 42666 is chunk time, / 2 just to be safe
            match select(
                Timer::after(Duration::from_micros(21332 / 2)),
                DAC_REQUEST.wait(),
            )
            .await
            {
                Either::First(()) => {
                    if !audio_is_track_loaded() {
                        return PlayOutcome::Next;
                    }
                    if let Ok(avail) = transfer.available() {
                        if avail >= sd::AUDIO_CHUNK_BYTES {
                            // If everything goes well, we can push new data
                            let chunk = match audio_wait_for_chunk().await {
                                Ok(c) => c,
                                Err(_e) => {
                                    // Stop playback as error only occus when TRACK_LOADED is false
                                    info!("[DAC] Audio wait for chunk is err");
                                    return PlayOutcome::Stop;
                                }
                            };
                            let data = chunk.as_slice();
                            match transfer.push(data) {
                                Ok(_) => {}
                                Err(_) => {
                                    // Known esp-hal bug: available() sticks at 0
                                    // after a non-aligned push. Recreate the transfer.
                                    core::mem::drop(transfer);
                                    transfer = self
                                        .i2s
                                        .write_dma_circular(self.tx_buffer)
                                        .expect("failed to restart I2S DMA transfer");
                                }
                            }
                            return_audio_chunk(chunk).await;
                        }
                    } else {
                        core::mem::drop(transfer);
                        transfer = self
                            .i2s
                            .write_dma_circular(self.tx_buffer)
                            .expect("failed to restart I2S DMA transfer");
                    }
                }
                Either::Second(DacRequest::Pause) => {
                    core::mem::drop(transfer);
                    DAC_PAUSED.store(true, Ordering::Relaxed);
                    match self.wait_paused().await {
                        PauseOutcome::Resume => {
                            DAC_PAUSED.store(false, Ordering::Relaxed);
                            transfer = self
                                .i2s
                                .write_dma_circular(self.tx_buffer)
                                .expect("failed to restart I2S DMA transfer");
                        }
                        PauseOutcome::Stop => return PlayOutcome::Stop,
                        PauseOutcome::SwitchTo(p, n) => return PlayOutcome::SwitchTo(p, n),
                        PauseOutcome::Next => return PlayOutcome::Next,
                    }
                }
                Either::Second(DacRequest::Stop) => return PlayOutcome::Stop,
                Either::Second(DacRequest::Start(p, n)) => return PlayOutcome::SwitchTo(p, n),
                Either::Second(DacRequest::StartQueue) => {
                    DAC_RESPONSE.signal(DacResponse::Error(DacError::AlreadyPlaying));
                }
                Either::Second(DacRequest::QueueAdd(path, name)) => {
                    match self.queue.push_back((path, name)) {
                        Ok(()) => DAC_RESPONSE.signal(DacResponse::AddedToQueue),
                        Err(_n) => DAC_RESPONSE.signal(DacResponse::Error(DacError::QueueFull)),
                    }
                }
                Either::Second(DacRequest::Play) => {
                    DAC_RESPONSE.signal(DacResponse::Error(DacError::AlreadyPlaying));
                }
                Either::Second(DacRequest::GetTrackInfo) => {
                    let (path, name) = self.current.clone().unwrap();
                    DAC_RESPONSE.signal(DacResponse::TrackInfo(Ok((path, name, self.tracknumber))));
                }
                Either::Second(DacRequest::GetStatus) => {
                    DAC_RESPONSE.signal(DacResponse::Status {
                        loaded: true,
                        paused: false,
                    });
                }
                Either::Second(DacRequest::ClearQueue) => self.queue.clear(),
                Either::Second(DacRequest::GetQueue) => {
                    // TODO: no DacResponse variant carries queue contents yet.
                }
                Either::Second(DacRequest::Skip) => {
                    return PlayOutcome::Next;
                }
            }
        }
    }

    /// Paused: file stays open, no chunks are read, just waits for a control signal.
    /// Signals Paused on entry so pause() has something to await, and Playing
    /// when resumed so play() does too.
    async fn wait_paused(&mut self) -> PauseOutcome {
        DAC_RESPONSE.signal(DacResponse::Paused);
        loop {
            if !DAC_PAUSED.load(Ordering::Relaxed) {
                return PauseOutcome::Resume;
            }
            // Either wait for request to start, or poll DAC_PAUSED
            match select(DAC_REQUEST.wait(), Timer::after(Duration::from_millis(100))).await {
                Either::First(req) => match req {
                    DacRequest::Play => {
                        DAC_RESPONSE.signal(DacResponse::Playing);
                        return PauseOutcome::Resume;
                    }
                    DacRequest::Stop => return PauseOutcome::Stop,
                    DacRequest::Start(p, n) => return PauseOutcome::SwitchTo(p, n),
                    DacRequest::StartQueue => {
                        DAC_RESPONSE.signal(DacResponse::Error(DacError::AlreadyPlaying));
                    }
                    DacRequest::QueueAdd(path, name) => match self.queue.push_back((path, name)) {
                        Ok(()) => DAC_RESPONSE.signal(DacResponse::AddedToQueue),
                        Err(_n) => DAC_RESPONSE.signal(DacResponse::Error(DacError::QueueFull)),
                    },
                    DacRequest::Pause => {
                        DAC_RESPONSE.signal(DacResponse::Error(DacError::UnknownResponse));
                    }
                    DacRequest::GetTrackInfo => {
                        let (p, n) = self.current.clone().unwrap();
                        DAC_RESPONSE.signal(DacResponse::TrackInfo(Ok((p, n, self.tracknumber))));
                    }
                    DacRequest::GetStatus => {
                        DAC_RESPONSE.signal(DacResponse::Status {
                            loaded: true,
                            paused: true,
                        });
                    }
                    DacRequest::ClearQueue => self.queue.clear(),
                    DacRequest::GetQueue => {
                        // TODO: no DacResponse variant carries queue contents yet.
                    }
                    DacRequest::Skip => return PauseOutcome::Next,
                },
                Either::Second(_) => {}
            }
        }
    }
}
