use embassy_time::Timer;
use embedded_hal_async::spi::SpiBus;
use esp_hal::Async;
use esp_hal::gpio::Output;
use esp_hal::spi::master::Spi;

use embedded_graphics::framebuffer::{Framebuffer, buffer_size};
use embedded_graphics::pixelcolor::Rgb565;
use embedded_graphics::pixelcolor::raw::{BigEndian, RawU16};

pub type FbType =
    Framebuffer<Rgb565, RawU16, BigEndian, 128, 160, { buffer_size::<Rgb565>(128, 160) }>;

pub struct Display {
    spi: Spi<'static, Async>,
    dc: Output<'static>,
    rst: Output<'static>,
    cs: Output<'static>,
}

impl Display {
    pub fn new(
        spi: Spi<'static, Async>,
        dc: Output<'static>,
        rst: Output<'static>,
        cs: Output<'static>,
    ) -> Self {
        Self { spi, dc, rst, cs }
    }

    pub async fn init_display(&mut self) {
        self.cs.set_low();

        self.rst.set_low();
        Timer::after_millis(10).await;
        self.rst.set_high();
        Timer::after_millis(120).await;

        self.write_command(0x01, &[]).await; // SWRESET
        Timer::after_millis(150).await;
        self.write_command(0x11, &[]).await; // SLPOUT
        Timer::after_millis(120).await;
        self.write_command(0x3A, &[0x05]).await; // COLMOD: 16-bit RGB565
        self.write_command(0x36, &[0x00]).await; // MADCTL
        self.write_command(0x29, &[]).await; // DISPON
        self.write_command(0x20, &[]).await; // INVOFF
        Timer::after_millis(10).await;
    }

    async fn write_command(&mut self, cmd: u8, args: &[u8]) {
        self.dc.set_low();
        SpiBus::write(&mut self.spi, &[cmd]).await.unwrap();

        if !args.is_empty() {
            self.dc.set_high();
            SpiBus::write(&mut self.spi, args).await.unwrap();
        }
    }

    pub async fn write_framebuf(&mut self, fb: &FbType) {
        self.write_command(0x2A, &[0x00, 0x00, 0x00, 0x7F]).await; // CASET
        self.write_command(0x2B, &[0x00, 0x00, 0x00, 0x9F]).await; // RASET

        self.dc.set_low();
        SpiBus::write(&mut self.spi, &[0x2C]).await.unwrap(); // RAMWR

        self.dc.set_high();
        SpiBus::write(&mut self.spi, fb.data()).await.unwrap();
    }
}
