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
use esp_backtrace as _;
use esp_hal::clock::CpuClock;
use esp_hal::timer::timg::TimerGroup;

use embassy_time::{Duration, Timer};
use esp_hal::{
    gpio::{Level, Output, OutputConfig},
    spi::{
        Mode,
        master::{Config, Spi},
    },
    time::Rate,
};

use embedded_graphics::framebuffer::Framebuffer;
use embedded_graphics::{
    mono_font::MonoTextStyle, mono_font::ascii::FONT_5X7, pixelcolor::Rgb565, prelude::*,
    text::Text,
};

use noise_perlin::perlin_2d;

use esp32_mp3_player::display::{Display, FbType};

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

    info!("Finished");

    let mut offsetx = 0f32;
    let mut offsety = 0f32;
    loop {
        let buf = fb.data_mut();

        for x in 0..128usize {
            for y in 0..160usize {
                let scale = 0.05;
                let n = perlin_2d(x as f32 * scale + offsetx, y as f32 * scale + offsety);
                let n01 = (n + 1.0) * 0.5;
                let val = (n01 * 255.0) as u8;

                let r5 = (val >> 3) as u16;
                let g6 = (val >> 2) as u16;
                let b5 = (val >> 3) as u16;
                let raw: u16 = (r5 << 11) | (g6 << 5) | b5;

                let idx = (y * 128 + x) * 2;
                // match this to the BO type param your Framebuffer was declared with
                let bytes = raw.to_be_bytes(); // BigEndian; use to_le_bytes() if you used LittleEndian
                buf[idx] = bytes[0];
                buf[idx + 1] = bytes[1];
            }
        }

        offsetx += 0.1f32;
        offsety += 0.1f32;
        disp.write_framebuf(&fb).await;
        Timer::after(Duration::from_millis(17)).await;
    }
}
