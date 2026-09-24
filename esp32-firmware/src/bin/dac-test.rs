#![no_std]
#![no_main]
#![deny(
    clippy::mem_forget,
    reason = "mem::forget is generally not safe to do with esp_hal types, especially those \
    holding buffers for the duration of a data transfer."
)]
#![deny(clippy::large_stack_frames)]

use defmt::*;
use defmt_rtt as _;
use esp_backtrace as _;

use embassy_executor::Spawner;
use embassy_time::{Duration, Timer};
use esp_hal::clock::CpuClock;
use esp_hal::dma_buffers;
use esp_hal::i2s::master::{Config as I2sConfig, DataFormat, I2s};
use esp_hal::time::Rate;
use esp_hal::timer::timg::TimerGroup;

use esp32_mp3_player::others::ROUNDABOUT_2MB;

esp_bootloader_esp_idf::esp_app_desc!();

#[allow(
    clippy::large_stack_frames,
    reason = "it's not unusual to allocate larger buffers etc. in main"
)]
#[esp_rtos::main]
async fn main(spawner: Spawner) -> ! {
    let config = esp_hal::Config::default().with_cpu_clock(CpuClock::max());
    let peripherals = esp_hal::init(config);

    let timg0 = TimerGroup::new(peripherals.TIMG0);
    let sw_interrupt =
        esp_hal::interrupt::software::SoftwareInterruptControl::new(peripherals.SW_INTERRUPT);
    esp_rtos::start(timg0.timer0, sw_interrupt.software_interrupt0);

    info!("Embassy initialized!");
    let _ = spawner;

    // Parse the embedded WAV first — need sample rate before configuring I2S.
    let (sample_rate, bits, channels, pcm) = parse_wav(ROUNDABOUT_2MB);
    info!(
        "WAV: {}Hz, {}bit, {}ch, {} bytes",
        sample_rate,
        bits,
        channels,
        pcm.len()
    );

    // I2S setup — pins from your DAC wiring
    let lrck = peripherals.GPIO21;
    let din = peripherals.GPIO47;
    let bck = peripherals.GPIO48;

    let (mut i2s_tx_buffer, i2s_tx_descriptors, _, _) = dma_buffers!(4 * 4092, 0);

    let i2s_config = I2sConfig::new_tdm_philips()
        .with_sample_rate(Rate::from_hz(sample_rate))
        .with_data_format(DataFormat::Data16Channel16);

    let i2s = I2s::new(peripherals.I2S0, peripherals.DMA_CH2, i2s_config)
        .unwrap()
        .into_async();

    let mut i2s_tx = i2s
        .i2s_tx
        .with_bclk(bck)
        .with_ws(lrck)
        .with_dout(din)
        .build(i2s_tx_descriptors);

    // Prime the DMA buffer with the first chunk before starting the
    // circular transfer.
    let initial_len = pcm.len().min(i2s_tx_buffer.len());
    i2s_tx_buffer[..initial_len].copy_from_slice(&pcm[..initial_len]);

    info!("DMA buffer size = {}", i2s_tx_buffer.len());

    let mut transfer = i2s_tx
        .write_dma_circular(&mut i2s_tx_buffer)
        .expect("failed to start I2S DMA transfer");

    let mut sent = initial_len;

    loop {
        let available = match transfer.available() {
            Ok(avail) => avail,
            Err(_) => {
                info!("I2S transfer errored, recreating");
                core::mem::drop(transfer);
                transfer = i2s_tx.write_dma_circular(&mut i2s_tx_buffer).unwrap();
                sent = 0;
                continue;
            }
        };

        if sent < pcm.len() && available > 0 {
            let remaining = pcm.len() - sent;
            let chunk_len = available.min(remaining);
            match transfer.push(&pcm[sent..sent + chunk_len]) {
                Ok(_) => sent += chunk_len,
                Err(_) => {
                    info!("push failed, recreating transfer");
                    core::mem::drop(transfer);
                    transfer = i2s_tx.write_dma_circular(&mut i2s_tx_buffer).unwrap();
                    sent = 0;
                    continue;
                }
            }
        } else if sent >= pcm.len() {
            info!("Playback complete, looping");
            sent = 0;
        }

        Timer::after(Duration::from_millis(5)).await;
    }
}

fn parse_wav(wav: &[u8]) -> (u32, u16, u16, &[u8]) {
    defmt::assert_eq!(&wav[0..4], b"RIFF");
    defmt::assert_eq!(&wav[8..12], b"WAVE");

    let mut pos = 12;
    let mut sample_rate = 0u32;
    let mut bits_per_sample = 0u16;
    let mut channels = 0u16;

    loop {
        defmt::assert!(
            pos + 8 <= wav.len(),
            "ran off end of WAV data looking for chunks, pos={}, len={}",
            pos,
            wav.len()
        );

        let chunk_id = &wav[pos..pos + 4];
        let chunk_size = u32::from_le_bytes(wav[pos + 4..pos + 8].try_into().unwrap()) as usize;

        info!(
            "chunk {:?} size {} at pos {}",
            core::str::from_utf8(chunk_id).unwrap_or("????"),
            chunk_size,
            pos
        );

        if chunk_id == b"fmt " {
            defmt::assert!(pos + 24 <= wav.len(), "fmt chunk truncated");
            channels = u16::from_le_bytes(wav[pos + 10..pos + 12].try_into().unwrap());
            sample_rate = u32::from_le_bytes(wav[pos + 12..pos + 16].try_into().unwrap());
            bits_per_sample = u16::from_le_bytes(wav[pos + 22..pos + 24].try_into().unwrap());
        } else if chunk_id == b"data" {
            let data_start = pos + 8;
            defmt::assert!(
                data_start + chunk_size <= wav.len(),
                "data chunk claims {} bytes but only {} available",
                chunk_size,
                wav.len() - data_start
            );
            return (
                sample_rate,
                bits_per_sample,
                channels,
                &wav[data_start..data_start + chunk_size],
            );
        }

        pos += 8 + chunk_size;
        if chunk_size % 2 == 1 {
            pos += 1;
        }
    }
}
