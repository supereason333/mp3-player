use defmt::*;
use defmt_rtt as _;

use embassy_rp::gpio::Input;

use embassy_futures::select::Either3;
use embassy_futures::select::select3;

use embassy_sync::blocking_mutex::raw::CriticalSectionRawMutex;
use embassy_sync::signal::Signal;

use embassy_time::Timer;

pub static NAV_EVENT: Signal<CriticalSectionRawMutex, NavEvent> = Signal::new();

pub enum NavEvent {
    Up,
    Down,
    Select,
}

async fn debounced_press(btn: &mut Input<'_>) {
    btn.wait_for_falling_edge().await; // assumes active-low with pull-up
    Timer::after_millis(30).await; // debounce settle
    if btn.is_low() {
        // confirmed press — wait for release before returning, to avoid re-trigger
        btn.wait_for_rising_edge().await;
    }
}

#[embassy_executor::task]
pub async fn input_task(
    mut btn_up: Input<'static>,
    mut btn_down: Input<'static>,
    mut btn_select: Input<'static>,
) {
    info!("[Input] Input task spawned");

    loop {
        match select3(
            debounced_press(&mut btn_up),
            debounced_press(&mut btn_down),
            debounced_press(&mut btn_select),
        )
        .await
        {
            Either3::First(_) => NAV_EVENT.signal(NavEvent::Up),
            Either3::Second(_) => NAV_EVENT.signal(NavEvent::Down),
            Either3::Third(_) => NAV_EVENT.signal(NavEvent::Select),
        }
    }
}
