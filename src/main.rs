#![no_std]
#![no_main]

use defmt::*;
use defmt_rtt as _;
use panic_probe as _;

use embassy_executor::Spawner;

// HAL Imports
use embassy_rp::gpio;

#[embassy_executor::main]
async fn main(_spawner: Spawner) {
    let p = embassy_rp::init(Default::default());
    info!("Start");
    let _led_pin = gpio::Output::new(p.PIN_25, gpio::Level::High);
    loop {}
}
