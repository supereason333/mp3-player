#![no_std]
#![no_main]
#![deny(
    clippy::mem_forget,
    reason = "mem::forget is generally not safe to do with esp_hal types, especially those \
    holding buffers for the duration of a data transfer."
)]
#![deny(clippy::large_stack_frames)]

use defmt::info;
use defmt_rtt as _;
use esp_backtrace as _;
use esp_hal::gpio::{Output, OutputConfig};

use embassy_executor::Spawner;
use embassy_time::{Duration, Timer};
use esp_hal::clock::CpuClock;
use esp_hal::timer::timg::TimerGroup;

use esp_hal::{
    dma::{DmaRxBuf, DmaTxBuf},
    dma_buffers,
    gpio::{Input, InputConfig, Level, Pull},
    i2s::master::{Config as I2sConfig, DataFormat, I2s},
    spi::{
        Mode,
        master::{Config, Spi},
    },
    time::Rate,
};

use esp32_mp3_player::dac;
use esp32_mp3_player::display;
use esp32_mp3_player::input::input_task;
use esp32_mp3_player::sd::sd_task;
use esp32_mp3_player::ui;

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

    let config = esp_hal::Config::default().with_cpu_clock(CpuClock::max());
    let peripherals = esp_hal::init(config);

    let timg0 = TimerGroup::new(peripherals.TIMG0);
    let sw_interrupt =
        esp_hal::interrupt::software::SoftwareInterruptControl::new(peripherals.SW_INTERRUPT);
    esp_rtos::start(timg0.timer0, sw_interrupt.software_interrupt0);

    info!("Embassy initialized!");

    // Set up Display SPI
    let blk = Output::new(peripherals.GPIO21, Level::High, OutputConfig::default());
    let cs = Output::new(peripherals.GPIO47, Level::High, OutputConfig::default());
    let dc = Output::new(peripherals.GPIO48, Level::High, OutputConfig::default());
    let rst = Output::new(peripherals.GPIO38, Level::High, OutputConfig::default());
    let mosi = peripherals.GPIO2;
    let sclk = peripherals.GPIO1;

    let display_spi = Spi::new(
        peripherals.SPI2,
        Config::default()
            .with_frequency(Rate::from_mhz(15))
            .with_mode(Mode::_0),
    )
    .unwrap()
    .with_sck(sclk)
    .with_mosi(mosi)
    .into_async();

    let mut disp = display::Display::new(display_spi, dc, rst, cs);

    disp.init_display().await;

    // Set up SD card SPIcs(cs);
    let miso = peripherals.GPIO11;
    let mosi = peripherals.GPIO10;
    let sclk = peripherals.GPIO9;
    let cs = peripherals.GPIO8;

    let dma_channel = peripherals.DMA_CH1;

    let (rx_buffer, rx_descriptors, tx_buffer, tx_descriptors) = dma_buffers!(32000);
    let dma_rx_buf = DmaRxBuf::new(rx_descriptors, rx_buffer).unwrap();
    let dma_tx_buf = DmaTxBuf::new(tx_descriptors, tx_buffer).unwrap();

    let sd_spi = Spi::new(
        peripherals.SPI3,
        Config::default()
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

    let sd_cs = Output::new(cs, Level::Low, OutputConfig::default());

    // I2S setup
    let lrck = peripherals.GPIO14;
    let din = peripherals.GPIO13;
    let bck = peripherals.GPIO12;

    let (i2s_tx_buffer, i2s_tx_descriptors, _, _) = dma_buffers!(4 * 4092, 0);

    let i2s_config = I2sConfig::new_tdm_philips()
        // .with_sample_rate(Rate::from_hz(sample_rate))
        .with_data_format(DataFormat::Data16Channel16);

    let i2s = I2s::new(peripherals.I2S0, peripherals.DMA_CH2, i2s_config)
        .unwrap()
        .into_async();

    let i2s_tx = i2s
        .i2s_tx
        .with_bclk(bck)
        .with_ws(lrck)
        .with_dout(din)
        .build(i2s_tx_descriptors);

    // Buttons
    let dial_down = Input::new(
        peripherals.GPIO6,
        InputConfig::default().with_pull(Pull::Up),
    );
    let dial_up = Input::new(
        peripherals.GPIO7,
        InputConfig::default().with_pull(Pull::Up),
    );
    let dial_select = Input::new(
        peripherals.GPIO15,
        InputConfig::default().with_pull(Pull::Up),
    );
    let btn_play = Input::new(
        peripherals.GPIO4,
        InputConfig::default().with_pull(Pull::Up),
    );
    let btn_back = Input::new(
        peripherals.GPIO5,
        InputConfig::default().with_pull(Pull::Up),
    );

    info!("Init finished!");

    spawner.spawn(sd_task(sd_spi, sd_cs).unwrap());

    spawner.spawn(input_task(dial_up, dial_down, dial_select, btn_play, btn_back).unwrap());

    spawner.spawn(dac::dac_task(i2s_tx, i2s_tx_buffer).unwrap());

    spawner.spawn(ui::ui_task(disp).unwrap());

    loop {
        Timer::after(Duration::from_secs(1)).await;
    }
    // for inspiration have a look at the examples at https://github.com/esp-rs/esp-hal/tree/esp-hal-v1.1.0/examples
}
