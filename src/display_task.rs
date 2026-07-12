use defmt::*;
use defmt_rtt as _;

// use heapless::Vec as HVec;

// use embedded_graphics::framebuffer::{Framebuffer, buffer_size};
// use embedded_graphics::pixelcolor::raw::{BigEndian, RawU16};
// use embedded_graphics::primitives::{PrimitiveStyleBuilder, Rectangle};
// use embedded_graphics::{
//     mono_font::MonoTextStyle, mono_font::ascii::FONT_10X20, pixelcolor::Rgb565, prelude::*,
//     text::Text,
// };

use embedded_graphics::framebuffer::Framebuffer;

use crate::display::{Display, FbType};
use crate::input::NAV_EVENT;
use crate::screens::{Screen, Transition, browser};
// use crate::sd::{DirListing, SD_REQUEST, SD_RESPONSE, SdRequest};

#[embassy_executor::task]
pub async fn display_task(mut display: Display) {
    info!("[Display] Display task spawned");
    let mut fb: FbType = Framebuffer::new();
    let mut screen = Screen::Browser(browser::BrowserScreen::new());
    screen.on_enter().await;

    loop {
        fb.data_mut().fill(0x00);
        screen.draw(&mut fb);
        display.write_framebuf(&fb).await;

        let event = NAV_EVENT.wait().await; // or select() with a ticker, per earlier answer

        match screen.on_event(event).await {
            Transition::Stay => {}
            Transition::GoTo(new_screen) => {
                screen = new_screen;
                screen.on_enter().await;
            }
        }
    }
}
