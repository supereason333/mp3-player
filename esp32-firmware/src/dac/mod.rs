// dac.rs
pub mod client;

use defmt::*;
use defmt_rtt as _;
use static_cell::StaticCell;

use embassy_sync::blocking_mutex::raw::CriticalSectionRawMutex;
use embassy_sync::signal::Signal;
use embassy_time::{Duration, Timer};

use embedded_sdmmc::ShortFileName;

use esp_hal::Async;
use esp_hal::i2s::master::I2sTx;

use crate::sd::DirPath;
use crate::sd::client::*;

static DAC_REQUEST: Signal<CriticalSectionRawMutex, DacRequest> = Signal::new();
static DAC_RESPONSE: Signal<CriticalSectionRawMutex, DacResponse> = Signal::new();

enum DacRequest {
    Start(DirPath, ShortFileName),
    Play,
    Pause,
    Stop,
    GetTrackInfo,
}

#[derive(defmt::Format)]
pub enum DacError {
    NoAudioLoaded,
    CantOpenFile,
    AlreadyPlaying,
    UnknownResponse,
}

enum DacResponse {
    Opened,
    Closed,
    Error(DacError),
    TrackInfo(Result<(DirPath, ShortFileName, i32), i32>),
}

pub const SAMPLE_RATE: u32 = 48_000;
pub const BIT_DEPTH: u32 = 16;

const DMA_BUFFER_SIZE: usize = 4 * 4092;

const I2S_CHUNK_BYTES: usize = crate::sd::AUDIO_CHUNK_FRAMES * 4;

static I2S_SCRATCH: StaticCell<[u8; I2S_CHUNK_BYTES]> = StaticCell::new();

#[embassy_executor::task]
pub async fn dac_task(
    mut i2s_tx: I2sTx<'static, Async>,
    tx_buffer: &'static mut [u8; DMA_BUFFER_SIZE],
) {
    info!("[DAC] DAC task spawned");

    let mut tracknumber: i32 = 0;

    let scratch: &'static mut [u8; I2S_CHUNK_BYTES] = I2S_SCRATCH.init([0u8; I2S_CHUNK_BYTES]);

    loop {
        // Wait for a Start request, opening the file via the SD client.
        let (path, name) = loop {
            match DAC_REQUEST.wait().await {
                DacRequest::Start(path, name) => break (path, name),
                DacRequest::GetTrackInfo => {
                    DAC_RESPONSE.signal(DacResponse::TrackInfo(Err(tracknumber)));
                }
                _ => {
                    DAC_RESPONSE.signal(DacResponse::Error(DacError::NoAudioLoaded));
                }
            }
        };
        DAC_RESPONSE.signal(DacResponse::Opened);
        tracknumber += 1;

        match audio_open(path.clone(), name.clone()).await {
            Ok((data_offset, data_size)) => {
                info!(
                    "[DAC] opened, {} bytes of PCM data at offset {}",
                    data_size, data_offset
                );
            }
            Err(()) => {
                error!("[DAC] Cant open file!");
                continue;
            }
        }

        // Prime the first chunk before starting the DMA transfer.
        let pending_chunk = match audio_read_chunk().await {
            Ok(chunk) => chunk,
            Err(_e) => {
                error!("[DAC] no data on first read, aborting playback");
                audio_close().await.ok();
                continue;
            }
        };

        pcm_bytes_to_i2s_bytes(pending_chunk, scratch);
        return_audio_chunk(pending_chunk).await;

        let mut transfer = match i2s_tx.write_dma_circular(tx_buffer) {
            Ok(t) => t,
            Err(e) => {
                error!("failed to start I2S DMA transfer");
                continue;
            }
        };

        let mut pending_offset = 0usize;
        let mut chunk_requested = false;

        'playback: loop {
            // Check for stop/new-start requests.
            if let Some(req) = DAC_REQUEST.try_take() {
                match req {
                    DacRequest::Stop => {
                        DAC_RESPONSE.signal(DacResponse::Closed);
                        audio_close().await.ok();
                        break 'playback;
                    }
                    DacRequest::Start(_, _) => {
                        DAC_RESPONSE.signal(DacResponse::Error(DacError::AlreadyPlaying))
                    }
                    DacRequest::GetTrackInfo => {
                        DAC_RESPONSE.signal(DacResponse::TrackInfo(Ok((
                            path.clone(),
                            name.clone(),
                            tracknumber,
                        ))));
                    }
                    DacRequest::Pause | DacRequest::Play => {
                        defmt::panic!("PAUSE PLAY NOT IMPLIMENTED!"); // TODO: PAUSE PLAY
                    }
                }
            }

            // Service the DMA transfer.
            match transfer.available() {
                Ok(avail) if avail > 0 && pending_offset < scratch.len() => {
                    let remaining = scratch.len() - pending_offset;
                    let push_len = avail.min(remaining);
                    match transfer.push(&scratch[pending_offset..pending_offset + push_len]) {
                        Ok(_) => pending_offset += push_len,
                        Err(_) => {
                            error!("[DAC] DMA push failed, recreating transfer");
                            core::mem::drop(transfer);
                            transfer = i2s_tx
                                .write_dma_circular(tx_buffer)
                                .expect("failed to restart I2S DMA transfer");
                            pending_offset = 0;
                        }
                    }
                }
                Err(_) => {
                    error!("[DAC] DMA available() errored, recreating transfer");
                    core::mem::drop(transfer);
                    transfer = i2s_tx
                        .write_dma_circular(tx_buffer)
                        .expect("failed to restart I2S DMA transfer");
                    pending_offset = 0;
                }
                _ => {}
            }

            // Scratch buffer exhausted — kick off/poll the next chunk.
            if pending_offset >= scratch.len() {
                if !chunk_requested {
                    audio_request_chunk().await;
                    chunk_requested = true;
                }

                if let Some(result) = audio_poll_chunk() {
                    chunk_requested = false;
                    match result {
                        Ok(chunk) => {
                            pcm_bytes_to_i2s_bytes(chunk, scratch);
                            return_audio_chunk(chunk).await;
                            pending_offset = 0;
                        }
                        Err(()) => {
                            info!("[DAC] playback ended (EOF or read error)");
                            audio_close().await.ok();
                            break 'playback;
                        }
                    }
                }
            }

            Timer::after(Duration::from_millis(2)).await;
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
