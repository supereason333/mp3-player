#![no_std]
#![no_main]

mod bmp820;
mod dac;
mod decoder;
mod display;
mod display_task;
mod input;
mod screens;
mod sd;

use defmt::*;
use defmt_rtt as _;

use panic_probe as _;

use embassy_executor::Spawner;

use embassy_time::Delay;

// HAL Imports
use embassy_rp::gpio::Level;
use embassy_rp::gpio::Output;
use embassy_rp::peripherals::{DMA_CH0, DMA_CH1, DMA_CH2, DMA_CH3, I2C0, PIO0};
use embassy_rp::pio::Pio;
use embassy_rp::pio_programs::i2s::{PioI2sOut, PioI2sOutProgram};
use embassy_rp::spi::Spi;
use embassy_rp::{bind_interrupts, dma, gpio, i2c, pio, spi};

use embedded_hal_bus::spi::ExclusiveDevice;

use crate::dac::dac_task;
// use crate::bmp820::BMP280;
use crate::display::Display;
use crate::display_task::display_task;
use crate::input::input_task;
use crate::sd::sd_task;

bind_interrupts!(struct Irqs {
    I2C0_IRQ => i2c::InterruptHandler<I2C0>;
    PIO0_IRQ_0 => pio::InterruptHandler<PIO0>;
    DMA_IRQ_0 => dma::InterruptHandler<DMA_CH0>, dma::InterruptHandler<DMA_CH1>, dma::InterruptHandler<DMA_CH2>, dma::InterruptHandler<DMA_CH3>;
});

const DISPLAY_FREQ: u32 = 32_000_000;
const BMP280_ADDR: u8 = 0x76;

#[embassy_executor::main]
async fn main(spawner: Spawner) {
    let p = embassy_rp::init(Default::default());
    info!("[Main] Start");
    let _led_pin = gpio::Output::new(p.PIN_25, gpio::Level::High);

    // Configure I2C
    // let sda = p.PIN_20;
    // let scl = p.PIN_21;
    // let config = embassy_rp::i2c::Config::default();
    // let bus: embassy_rp::i2c::I2c<I2C0, embassy_rp::i2c::Async> =
    //     embassy_rp::i2c::I2c::new_async(p.I2C0, scl, sda, Irqs, config);
    // let mut _bmp_280 = BMP280::new(bus, BMP280_ADDR).await;
    // info!("I2C Configured");
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
    // info!("Display Configured");
    let spi = embassy_rp::spi::Spi::new_txonly(p.SPI1, clk, mosi, p.DMA_CH0, Irqs, display_config);
    let mut display = Display::new(spi, dcx, rst, cs).await;
    display.init_display().await;
    // info!("Display Init");

    // buttons
    let mut _btn = gpio::Input::new(p.PIN_15, gpio::Pull::Up);
    let btn_up = gpio::Input::new(p.PIN_2, gpio::Pull::Up);
    let btn_down = gpio::Input::new(p.PIN_3, gpio::Pull::Up);
    let btn_ok = gpio::Input::new(p.PIN_4, gpio::Pull::Up);

    // SD card
    let miso = p.PIN_16;
    let cs_pin = Output::new(p.PIN_17, Level::High);
    let clk = p.PIN_18;
    let mosi = p.PIN_19;

    let mut config = spi::Config::default();
    config.frequency = 400_000;

    let spi_bus = Spi::new(p.SPI0, clk, mosi, miso, p.DMA_CH1, p.DMA_CH2, Irqs, config);

    let spi_device = match ExclusiveDevice::new(spi_bus, cs_pin, Delay) {
        Ok(device) => device,
        Err(_e) => defmt::panic!("Failed to get exclusive device"),
    };

    // I2S DAC setup
    let Pio {
        mut common, sm0, ..
    } = Pio::new(p.PIO0, Irqs);

    let bit_clock_pin = p.PIN_26; // BCK
    let left_right_clock_pin = p.PIN_27; // LRCK (word select)
    let data_pin = p.PIN_28; // DIN

    let program = PioI2sOutProgram::new(&mut common);
    let i2s = PioI2sOut::new(
        &mut common,
        sm0,
        p.DMA_CH3,
        Irqs,
        data_pin,
        bit_clock_pin,
        left_right_clock_pin,
        dac::SAMPLE_RATE,
        dac::BIT_DEPTH,
        &program,
    );
    // i2s.start();

    info!("[Main] Spawning");
    _ = spawner.spawn(input_task(btn_up, btn_down, btn_ok));

    _ = spawner.spawn(display_task(display));

    _ = spawner.spawn(sd_task(spi_device));

    _ = spawner.spawn(dac_task(i2s))
}
