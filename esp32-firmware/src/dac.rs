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

use crate::sd::DirPath;
use crate::sd::client::*;

pub static DAC_REQUEST: Signal<CriticalSectionRawMutex, DacRequest> = Signal::new();

pub enum DacRequest {
    Start(DirPath, ShortFileName),
    Play,
    Pause,
    Stop,
}

pub const SAMPLE_RATE: u32 = 48_000;
pub const BIT_DEPTH: u32 = 16;

pub const DMA_BUFFER_SIZE: usize = 4 * 4092;

const I2S_CHUNK_BYTES: usize = crate::sd::AUDIO_CHUNK_FRAMES * 4;

static I2S_SCRATCH: StaticCell<[u8; I2S_CHUNK_BYTES]> = StaticCell::new();

#[embassy_executor::task]
pub async fn dac_task(
    mut i2s_tx: I2sTx<'static, Async>,
    tx_buffer: &'static mut [u8; DMA_BUFFER_SIZE],
) {
    info!("[DAC] DAC task spawned");

    let scratch: &'static mut [u8; I2S_CHUNK_BYTES] = I2S_SCRATCH.init([0u8; I2S_CHUNK_BYTES]);

    loop {
        // Wait for a Start request, opening the file via the SD client.
        let (path, name) = loop {
            match DAC_REQUEST.wait().await {
                DacRequest::Start(path, name) => break (path, name),
                _ => {} // ignore Play/Pause/Stop while idle
            }
        };

        match audio_open(path, name).await {
            Ok((data_offset, data_size)) => {
                info!(
                    "[DAC] opened, {} bytes of PCM data at offset {}",
                    data_size, data_offset
                );
            }
            Err(()) => {
                error!("[DAC] failed to open file (no card, or read error)");
                continue;
            }
        }

        // Prime the first chunk before starting the DMA transfer.
        let pending_chunk = match audio_read_chunk().await {
            Ok(chunk) => chunk,
            Err(()) => {
                error!("[DAC] no data on first read, aborting playback");
                audio_close().await.ok();
                continue;
            }
        };

        pcm_bytes_to_i2s_bytes(pending_chunk, scratch);
        return_audio_chunk(pending_chunk).await;

        let mut transfer = i2s_tx
            .write_dma_circular(tx_buffer)
            .expect("failed to start I2S DMA transfer");

        let mut pending_offset = 0usize;
        let mut chunk_requested = false;

        'playback: loop {
            // Check for stop/new-start requests.
            if let Some(req) = DAC_REQUEST.try_take() {
                match req {
                    DacRequest::Start(_, _) | DacRequest::Stop => {
                        info!("[DAC] Stop/new-Start received, ending playback");
                        audio_close().await.ok();
                        break 'playback;
                    }
                    DacRequest::Pause | DacRequest::Play => {}
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
