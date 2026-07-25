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
    Play,
    For,
    Rev,
    Back,
}

enum DialEvent {
    Select,
    Up,
    Down,
    Up_, // when select is also pressed
    Down_,
}

async fn debounced_press(btn: &mut Input<'_>) {
    btn.wait_for_falling_edge().await; // assumes active-low with pull-up
    Timer::after_millis(80).await; // debounce settle
}

async fn dial_action(
    up: &mut Input<'_>,
    down: &mut Input<'_>,
    btn_select: &mut Input<'_>,
) -> DialEvent {
    match select3(
        up.wait_for_falling_edge(),
        down.wait_for_falling_edge(),
        btn_select.wait_for_rising_edge(),
    )
    .await
    {
        Either3::First(()) => {
            if btn_select.is_high() {
                Timer::after_millis(80).await;
                return DialEvent::Up;
            } else {
                Timer::after_millis(80).await;
                return DialEvent::Up_;
            }
        }
        Either3::Second(()) => {
            if btn_select.is_high() {
                Timer::after_millis(80).await;
                return DialEvent::Down;
            } else {
                Timer::after_millis(80).await;
                return DialEvent::Down_;
            }
        }
        Either3::Third(()) => {
            Timer::after_millis(80).await;
            return DialEvent::Select;
        }
    }
}

#[embassy_executor::task]
pub async fn input_task(
    mut dial_up: Input<'static>,
    mut dial_down: Input<'static>,
    mut dial_select: Input<'static>,
    mut btn_play: Input<'static>,
    mut btn_back: Input<'static>,
) {
    info!("[Input] Input task spawned");

    let mut last_select_held = false;
    loop {
        match select3(
            dial_action(&mut dial_up, &mut dial_down, &mut dial_select),
            debounced_press(&mut btn_play),
            debounced_press(&mut btn_back),
        )
        .await
        {
            Either3::First(dial_event) => match dial_event {
                DialEvent::Select => {
                    if !last_select_held {
                        NAV_EVENT.signal(NavEvent::Select)
                    }
                    last_select_held = false;
                }
                DialEvent::Up => {
                    NAV_EVENT.signal(NavEvent::Up);
                    last_select_held = false
                }
                DialEvent::Up_ => {
                    NAV_EVENT.signal(NavEvent::For);
                    last_select_held = true
                }
                DialEvent::Down => {
                    NAV_EVENT.signal(NavEvent::Down);
                    last_select_held = false
                }
                DialEvent::Down_ => {
                    NAV_EVENT.signal(NavEvent::Rev);
                    last_select_held = true
                }
            },
            Either3::Second(()) => NAV_EVENT.signal(NavEvent::Play),
            Either3::Third(()) => NAV_EVENT.signal(NavEvent::Back),
        }
    }
}
