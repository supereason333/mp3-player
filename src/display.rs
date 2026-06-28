use display_interface_spi::SPIInterface;
use embassy_rp::gpio::Output;
use embassy_rp::peripherals::SPI1;
use embassy_rp::spi::{Blocking, Spi};
use embedded_hal_bus::spi::ExclusiveDevice;
use mipidsi::{Builder, Display, models::ST7735s};

// Type alias so you never have to write this again
type DisplaySpi =
    ExclusiveDevice<Spi<'static, SPI1, Blocking>, Output<'static>, embedded_hal_bus::spi::NoDelay>;
type DisplayDi = SPIInterface<DisplaySpi, Output<'static>>;
pub type ST7735Display = Display<DisplayDi, ST7735s, Output<'static>>;

pub fn build_display(
    spi: Spi<'static, SPI1, Blocking>,
    cs: Output<'static>,
    dcx: Output<'static>,
    rst: Output<'static>,
) -> ST7735Display {
    let spi_dev = ExclusiveDevice::new_no_delay(spi, cs).unwrap();
    let di = SPIInterface::new(spi_dev, dcx);
    Builder::new(ST7735s, di)
        .display_size(128, 160)
        .reset_pin(rst)
        .init(&mut embassy_time::Delay)
        .unwrap()
}
