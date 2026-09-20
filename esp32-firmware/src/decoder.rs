use core::sync::atomic::{AtomicBool, Ordering};

use defmt::*;
use defmt_rtt as _;

use embassy_time::{Duration, Timer};
use heapless::spsc::Consumer;
use nanomp3::{Decoder, MAX_SAMPLES_PER_FRAME};

use crate::sd::AUDIO_CHUNK_BYTES;
use crate::sd::client::{audio_send_filled, audio_wait_for_empty_chunk};

/// Controls whether the decoder is actively pulling/decoding.
pub static DECODER_ACTIVE: AtomicBool = AtomicBool::new(false);

// Recommend size from nanomp3 docs
const READ_WINDOW: usize = 16 * 1024;

#[embassy_executor::task]
pub async fn decoder_task(mut consumer: Consumer<'static, u8>) {
    info!("[DECODER] Decoder task spawned");

    let mut decoder = Decoder::new();

    // Local buffer
    let mut window = [0u8; READ_WINDOW];
    let mut len = 0usize;

    let mut pcm_f32 = [0f32; MAX_SAMPLES_PER_FRAME];

    // Output chunk being assembled, plus how much of it is filled.
    let mut out_buf: Option<crate::sd::AudioChunk> = None;
    let mut out_pos = 0usize;

    let mut was_active = false;

    loop {
        let active = DECODER_ACTIVE.load(Ordering::Relaxed);

        if active && !was_active {
            // Drains ring
            info!("[DECODER] New track — draining stale ring data");
            while consumer.dequeue().is_some() {}
            len = 0;
            decoder = Decoder::new();
        }
        was_active = active;

        if !active {
            Timer::after(Duration::from_millis(10)).await;
            continue;
        }

        while len < window.len() {
            match consumer.dequeue() {
                Some(byte) => {
                    window[len] = byte;
                    len += 1;
                }
                None => break, // ring temporarily empty
            }
        }

        if len == 0 {
            Timer::after(Duration::from_millis(5)).await;
            continue;
        }

        let (consumed, frame_info) = decoder.decode(&window[..len], &mut pcm_f32);

        if let Some(info) = frame_info {
            let total_samples = info.samples_produced * info.channels.num() as usize;

            let buf_len = match &mut out_buf {
                Some(b) => b.len(),
                None => {
                    out_buf = Some(audio_wait_for_empty_chunk().await);
                    out_pos = 0;
                    out_buf.as_mut().unwrap().len()
                }
            };

            for &sample in &pcm_f32[..total_samples] {
                let s16 = (sample.clamp(-1.0, 1.0) * i16::MAX as f32) as i16;
                let bytes = s16.to_le_bytes();

                if out_pos + 2 > buf_len {
                    audio_send_filled(out_buf.take().unwrap()).await;
                    out_buf = Some(audio_wait_for_empty_chunk().await);
                    out_pos = 0;
                }
                let b = out_buf.as_mut().unwrap();
                b[out_pos..out_pos + 2].copy_from_slice(&bytes);
                out_pos += 2;
            }
        }

        if consumed > 0 {
            window.copy_within(consumed..len, 0);
            len -= consumed;
        } else if frame_info.is_none() {
            // decode() made no progress at all — avoid a busy-spin.
            Timer::after(Duration::from_millis(1)).await;
        }
    }
}
