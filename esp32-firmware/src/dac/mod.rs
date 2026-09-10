// dac.rs
pub mod client;

use core::sync::atomic::{AtomicBool, Ordering};
use defmt::*;
use defmt_rtt as _;
use embassy_futures::select::Either;
use embassy_futures::select::select;
use heapless::Deque;
use static_cell::StaticCell;

use embassy_sync::blocking_mutex::raw::CriticalSectionRawMutex;
use embassy_sync::signal::Signal;
use embassy_time::{Duration, Timer};

use embedded_sdmmc::ShortFileName;

use esp_hal::Async;
use esp_hal::i2s::master::I2sTx;

use crate::dac::DacRequest::Start;
use crate::sd;
use crate::sd::DirPath;
use crate::sd::client::*;

static DAC_REQUEST: Signal<CriticalSectionRawMutex, DacRequest> = Signal::new();
static DAC_RESPONSE: Signal<CriticalSectionRawMutex, DacResponse> = Signal::new();

/// True whenever a track is open (playing or paused). Mirrors current.is_some().
static DAC_LOADED: AtomicBool = AtomicBool::new(false);
/// True only while genuinely paused (i.e. inside wait_paused()).
static DAC_PAUSED: AtomicBool = AtomicBool::new(false);

enum DacRequest {
    Start(DirPath, ShortFileName),
    Play,
    Pause,
    Stop,
    GetTrackInfo,
    GetQueue,
    ClearQueue,
    GetStatus,
}

enum DacResponse {
    Opened,
    Closed,
    Paused,
    Playing,
    Error(DacError),
    TrackInfo(Result<(DirPath, ShortFileName, i32), i32>),
    Status { loaded: bool, paused: bool },
}

#[derive(defmt::Format, Debug)]
pub enum DacError {
    NoAudioLoaded,
    CantOpenFile,
    AlreadyPlaying,
    UnknownResponse,
    Unimplimented,
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
                DacRequest::Play | DacRequest::Pause | DacRequest::Stop => {
                    DAC_RESPONSE.signal(DacResponse::Error(DacError::NoAudioLoaded));
                }
            }
        }
    }

    /// Has a track loaded (playing or paused). Returns once `self.current` is None.
    async fn playing(&mut self) {
        loop {
            let Some((path, name)) = self.current.clone() else {
                return;
            };

            if audio_open(path.clone(), name.clone()).await.is_err() {
                DAC_RESPONSE.signal(DacResponse::Error(DacError::CantOpenFile));
                return;
            }
            DAC_RESPONSE.signal(DacResponse::Opened);
            self.tracknumber += 1;

            match self.play_loaded().await {
                PlayOutcome::Stop => {
                    audio_close().await.ok();
                    DAC_RESPONSE.signal(DacResponse::Closed);
                    return;
                }
                PlayOutcome::Next => {
                    audio_close().await.ok();
                    match self.queue.pop_front() {
                        Some((p, n)) => self.current = Some((p, n)),
                        None => return,
                    }
                }
                PlayOutcome::SwitchTo(p, n) => {
                    audio_close().await.ok();
                    self.current = Some((p, n));
                }
            }
        }
    }

    async fn play_loaded(&mut self) -> PlayOutcome {
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
                    if let Ok(avail) = transfer.available() {
                        if avail >= sd::AUDIO_CHUNK_BYTES {
                            // If everything goes well, we can push new data
                            let chunk = match audio_wait_for_chunk().await {
                                Ok(c) => c,
                                Err(_e) => {
                                    // Stop playback as error only occus when TRACK_LOADED is false
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
                    }
                }
                Either::Second(DacRequest::Stop) => return PlayOutcome::Stop,
                Either::Second(DacRequest::Start(p, n)) => return PlayOutcome::SwitchTo(p, n),
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
                },
                Either::Second(_) => {}
            }
        }
    }
}

fn pcm_bytes_to_i2s_bytes(
    bytes: &[u8; crate::sd::AUDIO_CHUNK_BYTES],
    out: &mut [u8; I2S_CHUNK_BYTES],
) {
    for (i, chunk) in bytes.chunks_exact(2).enumerate() {
        let out_idx = i * 4;
        out[out_idx..out_idx + 2].copy_from_slice(chunk);
        out[out_idx + 2..out_idx + 4].copy_from_slice(chunk);
    }
}
