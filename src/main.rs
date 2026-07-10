#![no_std]
#![no_main]

mod bmp820;
mod display;
mod sd;

use defmt::*;
use defmt_rtt as _;
use embassy_futures::select::Either3;
use embassy_futures::select::select3;

use panic_probe as _;

use core::fmt::Write;
use heapless::String;

use embassy_executor::Spawner;

use embassy_time::Delay;
use embassy_time::Instant;
use embassy_time::Timer;

// HAL Imports
use embassy_rp::gpio;
use embassy_rp::gpio::Level;
use embassy_rp::gpio::{Input, Output};
use embassy_rp::i2c::InterruptHandler;
use embassy_rp::peripherals::I2C0;
use embassy_rp::spi;
use embassy_rp::spi::Spi;

use embedded_hal_bus::spi::ExclusiveDevice;

use embassy_sync::blocking_mutex::raw::CriticalSectionRawMutex;
use embassy_sync::signal::Signal;

use embedded_graphics::framebuffer::{Framebuffer, buffer_size};
use embedded_graphics::pixelcolor::raw::{BigEndian, RawU16};
// use embedded_graphics::primitives::{PrimitiveStyleBuilder, Rectangle};
use embedded_graphics::{
    mono_font::MonoTextStyle, mono_font::ascii::FONT_10X20, pixelcolor::Rgb565, prelude::*,
    text::Text,
};

use embedded_sdmmc::{SdCard, TimeSource, Timestamp, VolumeIdx, VolumeManager};

use crate::bmp820::BMP280;
use crate::display::Display;
use crate::sd::DummyTimesource;

embassy_rp::bind_interrupts!(struct Irqs {
    I2C0_IRQ => InterruptHandler<embassy_rp::peripherals::I2C0>;
});

const DISPLAY_FREQ: u32 = 32_000_000;
const BMP280_ADDR: u8 = 0x76;

static NAV_EVENT: Signal<CriticalSectionRawMutex, NavEvent> = Signal::new();

enum NavEvent {
    Up,
    Down,
    Select,
}

async fn debounced_press(btn: &mut Input<'_>) {
    btn.wait_for_falling_edge().await; // assumes active-low with pull-up
    Timer::after_millis(30).await; // debounce settle
    if btn.is_low() {
        // confirmed press — wait for release before returning, to avoid re-trigger
        btn.wait_for_rising_edge().await;
    }
}

#[embassy_executor::task]
async fn input_task(
    mut btn_up: Input<'static>,
    mut btn_down: Input<'static>,
    mut btn_select: Input<'static>,
) {
    let mut selected_index: usize = 0;
    loop {
        match select3(
            debounced_press(&mut btn_up),
            debounced_press(&mut btn_down),
            debounced_press(&mut btn_select),
        )
        .await
        {
            Either3::First(_) => {
                selected_index = selected_index.saturating_sub(1);
                // trigger UI redraw signal here
            }
            Either3::Second(_) => {
                selected_index += 1;
                // trigger UI redraw signal here
            }
            Either3::Third(_) => {
                // confirm/select action
            }
        }
    }
}

#[embassy_executor::main]
async fn main(_spawner: Spawner) {
    let p = embassy_rp::init(Default::default());
    info!("Start");
    let _led_pin = gpio::Output::new(p.PIN_25, gpio::Level::High);

    // Configure I2C
    let sda = p.PIN_20;
    let scl = p.PIN_21;
    let config = embassy_rp::i2c::Config::default();
    let bus: embassy_rp::i2c::I2c<I2C0, embassy_rp::i2c::Async> =
        embassy_rp::i2c::I2c::new_async(p.I2C0, scl, sda, Irqs, config);
    let mut bmp_280 = BMP280::new(bus, BMP280_ADDR).await;

    // Build and take ownership of display
    let cs = Output::new(p.PIN_13, Level::Low);
    let dcx = Output::new(p.PIN_7, Level::High);
    let rst = Output::new(p.PIN_9, Level::High);
    let mosi = p.PIN_11;
    let clk = p.PIN_10; // SCL / SCK
    let mut display_config = spi::Config::default();
    display_config.frequency = DISPLAY_FREQ;
    display_config.phase = spi::Phase::CaptureOnFirstTransition;
    display_config.polarity = spi::Polarity::IdleLow;

    let spi = embassy_rp::spi::Spi::new_txonly(p.SPI1, clk, mosi, p.DMA_CH0, display_config);
    let mut display = Display::new(spi, dcx, rst, cs).await;
    display.init_display().await;

    // button
    let mut btn = gpio::Input::new(p.PIN_15, gpio::Pull::Up);

    // SD card
    let miso = p.PIN_16;
    let cs_pin = Output::new(p.PIN_17, Level::High);
    let clk = p.PIN_18;
    let mosi = p.PIN_19;

    let mut config = spi::Config::default();
    config.frequency = 400_000;

    let spi_bus = Spi::new_blocking(p.SPI0, clk, mosi, miso, config);

    let spi_device =
        ExclusiveDevice::new(spi_bus, cs_pin, Delay).expect("Failed to get exclusive device");

    let sdcard = SdCard::new(spi_device, Delay);

    // Write total size of SD card
    info!("Init SD card controller and retrieve card size...");
    let sd_size = sdcard.num_bytes().expect("failed to get sdcard size");
    info!("card size is {} bytes", sd_size);

    // Open first volume of SD card
    let volume_mgr = VolumeManager::new(sdcard, DummyTimesource::default());
    let volume0 = volume_mgr
        .open_volume(VolumeIdx(0))
        .expect("failed to open volume");

    let root_dir = volume0.open_root_dir().expect("failed to open root dir");

    let my_file = root_dir
        .open_file_in_dir("file.txt", embedded_sdmmc::Mode::ReadOnly)
        .expect("failed to open file.txt file");

    // Read file
    while !my_file.is_eof() {
        let mut buffer = [0u8; 32];

        if let Ok(n) = my_file.read(&mut buffer) {
            if let Ok(s) = core::str::from_utf8(&buffer[..n]) {
                defmt::info!("{}", s);
            } else {
                defmt::info!("{:02x}", &buffer[..n]);
            }
        }
    }

    loop {
        btn.wait_for_low().await;
        let start = Instant::now();

        // info!("Button pressed");
        let temp = bmp_280.read_temp().await;
        // info!("Temp: {}", temp);
        let int_part = temp as i32;
        let frac_part = ((temp - int_part as f32) * 100.0) as u32;
        let mut text: String<32> = String::new();
        core::write!(text, "Temp:\n{}.{:02}", int_part, frac_part).unwrap();

        let mut fb: Framebuffer<
            Rgb565,
            RawU16,
            BigEndian,
            128,
            160,
            { buffer_size::<Rgb565>(128, 160) },
        > = Framebuffer::new();
        // fb.clear(Rgb565::BLACK).unwrap();
        fb.data_mut().fill(0x00);

        // let style = PrimitiveStyleBuilder::new()
        //     .stroke_color(Rgb565::CSS_DARK_SLATE_GRAY)
        //     .stroke_width(3)
        //     .fill_color(Rgb565::CSS_DARK_GRAY)
        //     .build();

        // Rectangle::new(Point::new(1, 1), Size::new(126, 158))
        //     .into_styled(style)
        //     .draw(&mut fb)
        //     .unwrap();

        let style = MonoTextStyle::new(&FONT_10X20, Rgb565::GREEN);
        Text::new(&text, Point::new(10, 20), style)
            .draw(&mut fb)
            .unwrap();

        display.write_framebuf(fb).await;

        info!("Frame: {}ms", &start.elapsed().as_millis());
        btn.wait_for_high().await;
    }
}
