use embassy_rp::gpio::Output;
use embassy_rp::peripherals::SPI1;
use embassy_rp::spi::{Async, Spi};
use embassy_time::Timer;

use embedded_graphics::framebuffer::{Framebuffer, buffer_size};
use embedded_graphics::pixelcolor::Rgb565;
use embedded_graphics::pixelcolor::raw::{BigEndian, RawU16};

use crate::input::{NAV_EVENT, NavEvent};

pub struct Display {
    spi: Spi<'static, SPI1, Async>,
    dc: Output<'static>,
    rst: Output<'static>,
    cs: Output<'static>,
}

impl Display {
    // Gives a new display struct
    pub async fn new(
        spi: Spi<'static, SPI1, Async>,
        dc: Output<'static>,
        rst: Output<'static>,
        cs: Output<'static>,
    ) -> Self {
        Self {
            spi: spi,
            dc: dc,
            rst: rst,
            cs: cs,
        }
    }
    // Initilize the display
    pub async fn init_display(&mut self) {
        self.cs.set_low();
        // Hardware reset
        self.rst.set_low();
        Timer::after_millis(10).await;
        self.rst.set_high();
        Timer::after_millis(120).await;

        self.write_command(0x01, &[]).await; // SWRESET software reset
        Timer::after_millis(150).await;

        self.write_command(0x11, &[]).await; // SLPOUT sleep out
        Timer::after_millis(120).await;

        self.write_command(0x3A, &[0x05]).await; // COLMOD 16bit color RGB565
        self.write_command(0x36, &[0x00]).await; // MADCTL memory access control
        self.write_command(0x29, &[]).await; // DISPON display on
        // self.write_command(0x36, &[0x08]).await; // MADCTL: BGR order
        self.write_command(0x20, &[]).await; // INVOFF
        // self.write_command(0x21, &[]).await; // INVON Invert colors
        Timer::after_millis(10).await;
    }
    // Write a command to the display
    pub async fn write_command(&mut self, cmd: u8, args: &[u8]) {
        self.dc.set_low();
        self.spi.write(&[cmd]).await.unwrap();
        if !args.is_empty() {
            self.dc.set_high();
            self.spi.write(args).await.unwrap();
        }
    }
    pub async fn write_framebuf(
        &mut self,
        fb: Framebuffer<Rgb565, RawU16, BigEndian, 128, 160, { buffer_size::<Rgb565>(128, 160) }>,
    ) {
        // Column address set (CASET) — x0=0, x1=127
        self.write_command(0x2A, &[0x00, 0x00, 0x00, 0x7F]).await;

        // Row address set (RASET) — y0=0, y1=159
        self.write_command(0x2B, &[0x00, 0x00, 0x00, 0x9F]).await;

        // Memory write (RAMWR) — followed by raw pixel data
        self.dc.set_low();
        self.spi.write(&[0x2C]).await.unwrap();

        // Blast entire framebuffer in one transfer
        self.dc.set_high();
        self.spi.write(fb.data()).await.unwrap();
    }
    pub async fn clear(&mut self) {
        let fb: Framebuffer<
            Rgb565,
            RawU16,
            BigEndian,
            128,
            160,
            { buffer_size::<Rgb565>(128, 160) },
        > = Framebuffer::new();
        self.write_framebuf(fb).await;
    }
}

#[embassy_executor::task]
pub async fn display_task(display: Display) {
    let mut selected_index: usize = 0;

    loop {
        let event = NAV_EVENT.wait().await;

        match event {
            NavEvent::Up => {}
            NavEvent::Down => {}
            NavEvent::Select => {}
        }
    }
}
