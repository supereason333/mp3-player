// dac.rs

use defmt::*;
use defmt_rtt as _;
use static_cell::StaticCell;

use embassy_sync::blocking_mutex::raw::CriticalSectionRawMutex;
use embassy_sync::signal::Signal;
use embassy_time::{Duration, Timer};

use embedded_sdmmc::ShortFileName;

use esp_hal::Async;
use esp_hal::i2s::master::I2sTx;

use crate::sd::*;

pub static DAC_REQUEST: Signal<CriticalSectionRawMutex, DacRequest> = Signal::new();

pub enum DacRequest {
    Start(DirPath, ShortFileName),
    Play,
    Pause,
    Stop,
}

pub const SAMPLE_RATE: u32 = 48_000;
pub const BIT_DEPTH: u32 = 16;

// Shared with main.rs so both sides agree on the exact array type.
pub const DMA_BUFFER_SIZE: usize = 4 * 4092;

const I2S_CHUNK_BYTES: usize = AUDIO_CHUNK_FRAMES * 4;

static I2S_SCRATCH: StaticCell<[u8; I2S_CHUNK_BYTES]> = StaticCell::new();

#[embassy_executor::task]
pub async fn dac_task(
    mut i2s_tx: I2sTx<'static, Async>,
    tx_buffer: &'static mut [u8; DMA_BUFFER_SIZE], // <-- fixed-size array reference, Sized
) {
    info!("[DAC] DAC task spawned");

    let scratch: &'static mut [u8; I2S_CHUNK_BYTES] = I2S_SCRATCH.init([0u8; I2S_CHUNK_BYTES]);

    loop {
        let (path, name) = loop {
            match DAC_REQUEST.wait().await {
                DacRequest::Start(path, name) => {
                    AUDIO_SD_REQUEST
                        .send(AudioSdRequest::Open(path.clone(), name.clone()))
                        .await;
                    match AUDIO_SD_RESPONSE.wait().await {
                        AudioSdResponse::Opened { data_size, .. } => {
                            info!("[DAC] opened, {} bytes", data_size);
                            break (path, name);
                        }
                        _ => {
                            error!("[DAC] failed to open file");
                            continue;
                        }
                    }
                }
                _ => {}
            }
        };
        let _ = (path, name);

        let mut transfer = i2s_tx
            .write_dma_circular(tx_buffer)
            .expect("failed to start I2S DMA transfer");

        AUDIO_SD_REQUEST.send(AudioSdRequest::ReadChunk).await;
        let mut pending_chunk = AUDIO_FILLED.receive().await;
        AUDIO_SD_REQUEST.send(AudioSdRequest::ReadChunk).await;

        pcm_bytes_to_i2s_bytes(pending_chunk, scratch);
        AUDIO_EMPTY.send(pending_chunk).await;

        let mut pending_offset = 0usize;
        let mut stop_playback = false;

        loop {
            if let Some(req) = DAC_REQUEST.try_take() {
                match req {
                    DacRequest::Start(_, _) | DacRequest::Stop => {
                        info!("[DAC] Stop/new-Start received, ending playback");
                        AUDIO_SD_REQUEST.send(AudioSdRequest::Close).await;
                        stop_playback = true;
                    }
                    DacRequest::Pause | DacRequest::Play => {}
                }
            }
            if stop_playback {
                break;
            }

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

            if pending_offset >= scratch.len() {
                if let Ok(buf) = AUDIO_FILLED.try_receive() {
                    pending_chunk = buf;
                    pcm_bytes_to_i2s_bytes(pending_chunk, scratch);
                    AUDIO_EMPTY.send(pending_chunk).await;
                    pending_offset = 0;
                    AUDIO_SD_REQUEST.send(AudioSdRequest::ReadChunk).await;
                }
            }

            if let Some(resp) = AUDIO_SD_RESPONSE.try_take() {
                match resp {
                    AudioSdResponse::Eof => {
                        info!("[DAC] Playback ended");
                        AUDIO_SD_REQUEST.send(AudioSdRequest::Close).await;
                        break;
                    }
                    AudioSdResponse::Error => {
                        error!("[DAC] Playback error");
                        AUDIO_SD_REQUEST.send(AudioSdRequest::Close).await;
                        break;
                    }
                    _ => {}
                }
            }

            Timer::after(Duration::from_millis(2)).await;
        }

        while let Ok(buf) = AUDIO_FILLED.try_receive() {
            AUDIO_EMPTY.send(buf).await;
        }
    }
}

fn pcm_bytes_to_i2s_bytes(bytes: &[u8; AUDIO_CHUNK_BYTES], out: &mut [u8; I2S_CHUNK_BYTES]) {
    for (i, chunk) in bytes.chunks_exact(2).enumerate() {
        let out_idx = i * 4;
        out[out_idx..out_idx + 2].copy_from_slice(chunk);
        out[out_idx + 2..out_idx + 4].copy_from_slice(chunk);
    }
}
