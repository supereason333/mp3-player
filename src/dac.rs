use defmt::*;

use core::mem;

use embassy_time::Instant;

use embassy_futures::select::{Either, select};

use embassy_rp::peripherals::PIO0;
use embassy_rp::pio_programs::i2s::{PioI2sOut, PioI2sOutProgram};

use embassy_sync::blocking_mutex::raw::CriticalSectionRawMutex;
use embassy_sync::signal::Signal;

use embedded_sdmmc::ShortFileName;

use crate::dac;
use crate::sd::{
    AUDIO_CHUNK_BYTES, AUDIO_CHUNK_FRAMES, AUDIO_SD_REQUEST, AUDIO_SD_RESPONSE, AudioSdRequest,
    AudioSdResponse, DirPath,
};

pub static DAC_REQUEST: Signal<CriticalSectionRawMutex, DacRequest> = Signal::new();

pub enum DacRequest {
    Start(DirPath, ShortFileName),
    Play,
    Pause,
    Stop,
}

pub const SAMPLE_RATE: u32 = 48_000;
pub const BIT_DEPTH: u32 = 16;

#[embassy_executor::task]
pub async fn dac_task(mut i2s: PioI2sOut<'static, PIO0, 0>) {
    info!("[DAC] DAC task spawned");
    i2s.start();
    loop {
        loop {
            match DAC_REQUEST.wait().await {
                DacRequest::Start(path, name) => {
                    AUDIO_SD_REQUEST
                        .send(AudioSdRequest::Open(path.clone(), name.clone()))
                        .await;
                    match AUDIO_SD_RESPONSE.wait().await {
                        AudioSdResponse::Opened {
                            data_offset,
                            data_size,
                        } => {
                            info!("[DAC] opened, {} bytes", data_size);
                            break;
                        }
                        _ => {
                            error!("[DAC] failed to open file");
                            continue; // back to outer loop, wait for next DacRequest
                        }
                    }
                }
                _ => {}
            }
        }
        info!("[DAC] Recieved DAC request");
        AUDIO_SD_REQUEST.send(AudioSdRequest::ReadChunk).await;
        let mut front_buf: [u32; AUDIO_CHUNK_FRAMES] = match AUDIO_SD_RESPONSE.wait().await {
            AudioSdResponse::Chunk(pcm) => pcm_bytes_to_i2s_frames(&pcm),
            _ => continue, // failed to get first chunk — bail back to outer loop
        };
        let mut back_buf: [u32; AUDIO_CHUNK_FRAMES] = [0u32; AUDIO_CHUNK_FRAMES];
        AUDIO_SD_REQUEST.send(AudioSdRequest::ReadChunk).await;
        let mut i = 0;
        loop {
            // let start = Instant::now();
            let future = i2s.write(&front_buf);
            // let write = start.elapsed().as_millis();

            match select(DAC_REQUEST.wait(), AUDIO_SD_RESPONSE.wait()).await {
                Either::Second(sd_response) => match sd_response {
                    AudioSdResponse::Chunk(pcm) => {
                        AUDIO_SD_REQUEST.send(AudioSdRequest::ReadChunk).await;
                        back_buf = pcm_bytes_to_i2s_frames(&pcm);
                    }
                    AudioSdResponse::Eof => {
                        info!("[DAC] Playback ended");
                        break;
                    }
                    AudioSdResponse::Error => {
                        info!("[DAC] Playback error");
                        break;
                    }
                    _ => {
                        break;
                    }
                },
                Either::First(dac_request) => match dac_request {
                    DacRequest::Start(path, name) => {
                        AUDIO_SD_REQUEST.send(AudioSdRequest::Close).await;
                        break;
                    }
                    DacRequest::Pause => {}
                    DacRequest::Play => {}
                    DacRequest::Stop => {
                        info!("DAC Stop recieved");
                        AUDIO_SD_REQUEST.send(AudioSdRequest::Close).await;
                        break;
                    }
                },
            }
            // let response = start.elapsed().as_millis() - write;
            future.await;
            // let future_await = start.elapsed().as_millis() - response;
            mem::swap(&mut back_buf, &mut front_buf);

            if i % 50 == 0 {
                // info!(
                //     "[DAC] write: {} ms, send: {} ms, response: {} ms, future await: {} ms",
                //     write, send, response, future_await
                // );
            }
            i += 1;
        }
    }
}

// fn pcm_bytes_to_i2s_frames(bytes: &[u8; AUDIO_CHUNK_BYTES]) -> [u32; AUDIO_CHUNK_FRAMES] {
//     // WAV PCM is little-endian 16-bit samples, interleaved L/R for stereo.
//     // I2S typically wants 32-bit frames (16-bit L in upper/lower half + 16-bit R).
//     let mut frames: [u32; AUDIO_CHUNK_FRAMES] = [0u32; AUDIO_CHUNK_FRAMES];
//     let mut chunks = bytes.chunks_exact(4); // 2 bytes L + 2 bytes R = 4 bytes/frame
//     let mut i = 0;
//     for chunk in &mut chunks {
//         let left = i16::from_le_bytes([chunk[0], chunk[1]]) as u16;
//         let right = i16::from_le_bytes([chunk[2], chunk[3]]) as u16;
//         let frame = ((left as u32) << 16) | (right as u32); // VERIFY: exact bit layout PioI2sOut expects
//         frames[i] = frame;
//         i += 1;
//     }
//     frames
// }

// Single channel audio
fn pcm_bytes_to_i2s_frames(bytes: &[u8; AUDIO_CHUNK_BYTES]) -> [u32; AUDIO_CHUNK_FRAMES] {
    let mut frames = [0u32; AUDIO_CHUNK_FRAMES];
    for (i, chunk) in bytes.chunks_exact(2).enumerate() {
        // 2 bytes per mono sample now, not 4
        let sample = i16::from_le_bytes([chunk[0], chunk[1]]) as u16;
        frames[i] = (sample as u32) | ((sample as u32) << 16); // duplicate into both L and R
    }
    frames
}
