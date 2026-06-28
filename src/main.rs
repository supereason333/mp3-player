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

// HAL Imports
use embassy_rp::gpio;
use embassy_rp::gpio::Level;
use embassy_rp::gpio::Output;
use embassy_rp::i2c::InterruptHandler;
use embassy_rp::peripherals::I2C0;
use embassy_rp::spi;

use embedded_graphics::{
    mono_font::MonoTextStyle,
    mono_font::ascii::FONT_10X20,
    pixelcolor::Rgb565,
    prelude::*,
    primitives::{Circle, PrimitiveStyle, PrimitiveStyleBuilder, Rectangle},
    text::Text,
};

use crate::bmp820::BMP280;
use crate::display::build_display;

embassy_rp::bind_interrupts!(struct Irqs {
    I2C0_IRQ => InterruptHandler<embassy_rp::peripherals::I2C0>;
});

const DISPLAY_FREQ: u32 = 64_000_000;

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
    let cs = Output::new(p.PIN_13, Level::High);
    let dcx = Output::new(p.PIN_7, Level::High);
    let rst = Output::new(p.PIN_9, Level::High);
    let mosi = p.PIN_11;
    let clk = p.PIN_10; // SCL / SCK
    let mut display_config = spi::Config::default();
    display_config.frequency = DISPLAY_FREQ;
    display_config.phase = spi::Phase::CaptureOnFirstTransition;
    display_config.polarity = spi::Polarity::IdleLow;

    let spi = embassy_rp::spi::Spi::new_blocking_txonly(p.SPI1, clk, mosi, display_config);
    let mut display: display::ST7735Display = build_display(spi, cs, dcx, rst);
    display.clear(Rgb565::BLACK).unwrap();

    // button
    let mut btn = gpio::Input::new(p.PIN_15, gpio::Pull::Up);

    loop {
        btn.wait_for_low().await;
        info!("Button pressed");
        let temp = bmp_280.read_temp().await;
        let int_part = temp as i32;
        let frac_part = ((temp - int_part as f32) * 100.0) as u32;
        let mut text: String<32> = String::new();
        core::write!(text, "Temp:\n{}.{:02}", int_part, frac_part).unwrap();

        display.clear(Rgb565::BLACK).unwrap();
        let style = MonoTextStyle::new(&FONT_10X20, Rgb565::GREEN);

        Text::new(&text, Point::new(20, 20), style)
            .draw(&mut display)
            .unwrap();
    }
}
