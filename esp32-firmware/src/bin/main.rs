#![no_std]
#![no_main]
#![deny(
    clippy::mem_forget,
    reason = "mem::forget is generally not safe to do with esp_hal types, especially those \
    holding buffers for the duration of a data transfer."
)]
#![deny(clippy::large_stack_frames)]

use defmt::info;
use embassy_executor::Spawner;
use embassy_time::{Duration, Timer};
use esp_hal::timer::timg::TimerGroup;
use esp_hal::{clock::CpuClock, gpio};
use panic_rtt_target as _;

use esp_hal::{
    delay::Delay,
    dma::{DmaRxBuf, DmaTxBuf},
    dma_buffers,
    gpio::{Input, InputConfig, Level, Pull},
    i2s::master::{Config as I2sConfig, DataFormat, I2s},
    main,
    spi::{
        Mode,
        master::{Config, Spi},
    },
    time::Rate,
};

// This creates a default app-descriptor required by the esp-idf bootloader.
// For more information see: <https://docs.espressif.com/projects/esp-idf/en/stable/esp32/api-reference/system/app_image_format.html#application-description>
esp_bootloader_esp_idf::esp_app_desc!();

#[allow(
    clippy::large_stack_frames,
    reason = "it's not unusual to allocate larger buffers etc. in main"
)]
#[esp_rtos::main]
async fn main(spawner: Spawner) -> ! {
    // generator version: 1.3.0
    // generator parameters: --chip esp32s3 -o unstable-hal -o embassy -o probe-rs -o defmt -o panic-rtt-target -o zed

    rtt_target::rtt_init_defmt!();

    let config = esp_hal::Config::default().with_cpu_clock(CpuClock::max());
    let peripherals = esp_hal::init(config);

    let timg0 = TimerGroup::new(peripherals.TIMG0);
    let sw_interrupt =
        esp_hal::interrupt::software::SoftwareInterruptControl::new(peripherals.SW_INTERRUPT);
    esp_rtos::start(timg0.timer0, sw_interrupt.software_interrupt0);

    info!("Embassy initialized!");

    let _ = spawner;

    // Set up Display SPI
    let sclk = peripherals.GPIO1;
    let mosi = peripherals.GPIO2;
    let cs = peripherals.GPIO3;
    let dma_channel = peripherals.DMA_CH0;

    let (_, _, tx_buffer, tx_descriptors) = dma_buffers!(32000);
    let dma_tx_buf = DmaTxBuf::new(tx_descriptors, tx_buffer).unwrap();

    let mut display_spi = Spi::new(
        peripherals.SPI2,
        Config::default()
            .with_frequency(Rate::from_khz(100))
            .with_mode(Mode::_0),
    )
    .unwrap()
    .with_sck(sclk)
    .with_mosi(mosi)
    .with_cs(cs)
    .with_dma(dma_channel)
    .into_async();

    // Set up SD card SPIcs(cs);
    let sclk = peripherals.GPIO4;
    let mosi = peripherals.GPIO5;
    let miso = peripherals.GPIO6;
    let cs = peripherals.GPIO7;
    let dma_channel = peripherals.DMA_CH1;

    let (rx_buffer, rx_descriptors, tx_buffer, tx_descriptors) = dma_buffers!(32000);
    let dma_rx_buf = DmaRxBuf::new(rx_descriptors, rx_buffer).unwrap();
    let dma_tx_buf = DmaTxBuf::new(tx_descriptors, tx_buffer).unwrap();

    let mut display_spi = Spi::new(
        peripherals.SPI3,
        Config::default()
            .with_frequency(Rate::from_khz(100))
            .with_mode(Mode::_0),
    )
    .unwrap()
    .with_sck(sclk)
    .with_mosi(mosi)
    .with_miso(miso)
    .with_cs(cs)
    .with_dma(dma_channel)
    .with_buffers(dma_rx_buf, dma_tx_buf)
    .into_async();

    // I2S setup
    let (mut tx_buffer, tx_descriptors, _, _) = dma_buffers!(4 * 4092, 0);

    let i2s_config = I2sConfig::new_tdm_philips()
        // .with_sample_rate(Rate::from_hz(sample_rate))
        .with_data_format(DataFormat::Data16Channel16);

    let i2s = I2s::new(peripherals.I2S0, peripherals.DMA_CH2, i2s_config).unwrap();

    let mut i2s_tx = i2s
        .i2s_tx
        .with_bclk(peripherals.GPIO13) // BCLK -> DAC's BCK
        .with_ws(peripherals.GPIO14) // WS   -> DAC's LRCK/WS
        .with_dout(peripherals.GPIO15) // DOUT -> DAC's DIN
        .build(tx_descriptors);

    // Buttons
    let dial_down = Input::new(
        peripherals.GPIO8,
        InputConfig::default().with_pull(Pull::Up),
    );
    let dial_up = Input::new(
        peripherals.GPIO9,
        InputConfig::default().with_pull(Pull::Up),
    );
    let dial_select = Input::new(
        peripherals.GPIO10,
        InputConfig::default().with_pull(Pull::Up),
    );
    let btn_play = Input::new(
        peripherals.GPIO11,
        InputConfig::default().with_pull(Pull::Up),
    );
    let btn_back = Input::new(
        peripherals.GPIO12,
        InputConfig::default().with_pull(Pull::Up),
    );

    info!("Init finished!");

    loop {
        Timer::after(Duration::from_secs(1)).await;
    }

    // for inspiration have a look at the examples at https://github.com/esp-rs/esp-hal/tree/esp-hal-v1.1.0/examples
}
