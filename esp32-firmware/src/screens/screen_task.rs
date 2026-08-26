use defmt::*;
use defmt_rtt as _;

use embedded_graphics::framebuffer::Framebuffer;

use crate::display::{Display, FbType};
use crate::input::NAV_EVENT;
use crate::screens::{Screen, Transition, browser};

#[embassy_executor::task]
pub async fn screen_task(mut display: Display) {
    info!("[Display] Display task spawned");

    let mut fb: FbType = Framebuffer::new();
    let mut screen = Screen::Browser(browser::BrowserScreen::new());
    screen.on_enter().await;

    loop {
        fb.data_mut().fill(0x00);
        screen.draw(&mut fb);
        display.write_framebuf(&fb).await;

        let event = NAV_EVENT.wait().await;
        match screen.on_event(event).await {
            Transition::Stay => {}
            Transition::GoTo(new_screen) => {
                screen.on_close().await;
                screen = new_screen;
                screen.on_enter().await;
            }
        }
    }
}
