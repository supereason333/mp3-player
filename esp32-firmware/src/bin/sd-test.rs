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
use embassy_executor::Spawner;
// use embedded_hal_bus::delay::NoDelay;
use embedded_hal_bus::spi::ExclusiveDevice;
use embedded_sdmmc::sdcard::SdCard;
use esp_backtrace as _;
use esp_hal::clock::CpuClock;
use esp_hal::delay::Delay;
use esp_hal::dma::{DmaRxBuf, DmaTxBuf};
use esp_hal::dma_buffers;
use esp_hal::gpio::{Level, Output, OutputConfig};
use esp_hal::spi::Mode;
use esp_hal::spi::master::{Config as SpiConfig, Spi};
use esp_hal::time::Rate;
use esp_hal::timer::timg::TimerGroup;

esp_bootloader_esp_idf::esp_app_desc!();

#[esp_rtos::main]
async fn main(_spawner: Spawner) -> ! {
    let config = esp_hal::Config::default().with_cpu_clock(CpuClock::max());
    let peripherals = esp_hal::init(config);
    let timg0 = TimerGroup::new(peripherals.TIMG0);
    let sw_interrupt =
        esp_hal::interrupt::software::SoftwareInterruptControl::new(peripherals.SW_INTERRUPT);
    esp_rtos::start(timg0.timer0, sw_interrupt.software_interrupt0);
    info!("Embassy initialized!");

    // SD card pins
    let miso = peripherals.GPIO11;
    let mosi = peripherals.GPIO10;
    let sclk = peripherals.GPIO9;
    let cs = peripherals.GPIO8;
    let dma_channel = peripherals.DMA_CH1;

    // CS must idle HIGH before the SPI peripheral is attached to any pins
    let sd_cs = Output::new(cs, Level::High, OutputConfig::default());

    let (rx_buffer, rx_descriptors, tx_buffer, tx_descriptors) = dma_buffers!(512);
    let dma_rx_buf = DmaRxBuf::new(rx_descriptors, rx_buffer).unwrap();
    let dma_tx_buf = DmaTxBuf::new(tx_descriptors, tx_buffer).unwrap();

    let spi_bus = Spi::new(
        peripherals.SPI3,
        SpiConfig::default()
            .with_frequency(Rate::from_khz(400))
            .with_mode(Mode::_0),
    )
    .unwrap()
    .with_sck(sclk)
    .with_mosi(mosi)
    .with_miso(miso)
    .with_dma(dma_channel)
    .with_buffers(dma_rx_buf, dma_tx_buf)
    .into_async();

    let spi_device = match ExclusiveDevice::new_no_delay(spi_bus, sd_cs) {
        Ok(d) => d,
        Err(_) => defmt::panic!("Failed to build exclusive SPI device"),
    };

    let sdcard = SdCard::new(spi_device, Delay::new());

    match sdcard.num_bytes() {
        Ok(size) => info!("[SD] card size is {} bytes", size),
        Err(e) => defmt::panic!("[SD] failed to read card size: {}", Debug2Format(&e)),
    }

    loop {
        embassy_time::Timer::after(embassy_time::Duration::from_secs(1)).await;
    }
}
