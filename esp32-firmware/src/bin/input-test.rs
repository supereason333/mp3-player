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
use esp_backtrace as _;

use embassy_executor::Spawner;
use esp_hal::clock::CpuClock;
use esp_hal::gpio::{Input, InputConfig, Pull};
use esp_hal::timer::timg::TimerGroup;

use esp32_mp3_player::input::{NAV_EVENT, NavEvent, input_task};

esp_bootloader_esp_idf::esp_app_desc!();

#[embassy_executor::task]
async fn monitor_task() {
    info!("[Monitor] Waiting on NAV_EVENT...");
    loop {
        let event = NAV_EVENT.wait().await;
        match event {
            NavEvent::Up => info!("[Monitor] Up"),
            NavEvent::Down => info!("[Monitor] Down"),
            NavEvent::Select => info!("[Monitor] Select"),
            NavEvent::Play => info!("[Monitor] Play"),
            NavEvent::For => info!("[Monitor] Fast-forward (select+up)"),
            NavEvent::Rev => info!("[Monitor] Rewind (select+down)"),
            NavEvent::Back => info!("[Monitor] Back"),
        }
    }
}

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

    let dial_down = Input::new(
        peripherals.GPIO6,
        InputConfig::default().with_pull(Pull::Up),
    );
    let dial_up = Input::new(
        peripherals.GPIO7,
        InputConfig::default().with_pull(Pull::Up),
    );
    let dial_select = Input::new(
        peripherals.GPIO15,
        InputConfig::default().with_pull(Pull::Up),
    );
    let btn_play = Input::new(
        peripherals.GPIO4,
        InputConfig::default().with_pull(Pull::Up),
    );
    let btn_back = Input::new(
        peripherals.GPIO5,
        InputConfig::default().with_pull(Pull::Up),
    );

    info!("Init finished, spawning tasks!");

    spawner.spawn(input_task(dial_up, dial_down, dial_select, btn_play, btn_back).unwrap());
    spawner.spawn(monitor_task().unwrap());

    loop {
        embassy_time::Timer::after(embassy_time::Duration::from_secs(60)).await;
    }
}
