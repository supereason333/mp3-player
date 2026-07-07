#![no_std]
#![no_main]

mod bmp820;
mod display;

use defmt::*;
use defmt_rtt as _;
use panic_probe as _;

use core::fmt::Write;
use heapless::String;

use embassy_executor::Spawner;

use embassy_time::Instant;

// HAL Imports
use embassy_rp::gpio;
use embassy_rp::gpio::Level;
use embassy_rp::gpio::Output;
use embassy_rp::i2c::InterruptHandler;
use embassy_rp::peripherals::I2C0;
use embassy_rp::spi;

use embedded_graphics::framebuffer::{Framebuffer, buffer_size};
use embedded_graphics::pixelcolor::raw::{LittleEndian, RawU16};
use embedded_graphics::{
    mono_font::MonoTextStyle, mono_font::ascii::FONT_10X20, pixelcolor::Rgb565, prelude::*,
    text::Text,
};

use crate::bmp820::BMP280;
use crate::display::Display;

embassy_rp::bind_interrupts!(struct Irqs {
    I2C0_IRQ => InterruptHandler<embassy_rp::peripherals::I2C0>;
});

const DISPLAY_FREQ: u32 = 32_000_000;

const BMP280_ADDR: u8 = 0x76;

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

    loop {
        // btn.wait_for_low().await;
        let start = Instant::now();

        // info!("Button pressed");
        let temp = bmp_280.read_temp().await;
        // info!("Temp: {}", temp);
        let int_part = temp as i32;
        let frac_part = ((temp - int_part as f32) * 100.0) as u32;
        let mut text: String<32> = String::new();
        core::write!(text, "Temp:\n{}.{:02}", int_part, frac_part).unwrap();

        let style = MonoTextStyle::new(&FONT_10X20, Rgb565::GREEN);

        let mut fb: Framebuffer<
            Rgb565,
            RawU16,
            LittleEndian,
            128,
            160,
            { buffer_size::<Rgb565>(128, 160) },
        > = Framebuffer::new();

        // Draw everything to RAM — instant, no SPI
        Text::new(&text, Point::new(10, 20), style)
            .draw(&mut fb)
            .unwrap();

        display.write_framebuf(fb).await;

        info!("Frame: {}ms", &start.elapsed().as_millis());
        // btn.wait_for_high().await;
    }
}
