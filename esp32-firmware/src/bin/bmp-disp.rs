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
use embassy_time::{Duration, Timer};
use embedded_graphics::framebuffer::Framebuffer;
use embedded_graphics::{image::Image, prelude::*};
use esp_backtrace as _;
use esp_hal::clock::CpuClock;
use esp_hal::timer::timg::TimerGroup;
use esp_hal::{
    gpio::{Level, Output, OutputConfig},
    spi::{
        Mode,
        master::{Config, Spi},
    },
    time::Rate,
};
use esp32_mp3_player::display::{Display, FbType};
use esp32_mp3_player::ui::assets::ALEX;
use tinybmp::Bmp;

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

    let mut disp = Display::new(display_spi, dc, rst, cs);
    disp.init_display().await;

    let mut fb: FbType = Framebuffer::new();

    // Decode the embedded BMP. This parses the header/palette but does NOT
    // copy pixel data — Bmp borrows straight from ALEX_BMP, so no heap needed.
    let bmp = match Bmp::<embedded_graphics::pixelcolor::Rgb565>::from_slice(ALEX) {
        Ok(bmp) => bmp,
        Err(e) => {
            error!("Failed to parse BMP: {:?}", Debug2Format(&e));
            loop {
                Timer::after(Duration::from_secs(1)).await;
            }
        }
    };

    info!("BMP loaded: {}x{}", bmp.size().width, bmp.size().height);

    // Clear the framebuffer, then draw the image at the origin.
    fb.data_mut().fill(0);
    let image = Image::new(&bmp, Point::zero());
    image.draw(&mut fb).unwrap();

    info!("Drew BMP to framebuffer, pushing to display");
    disp.write_framebuf(&fb).await;

    loop {
        Timer::after(Duration::from_secs(1)).await;
    }
}
