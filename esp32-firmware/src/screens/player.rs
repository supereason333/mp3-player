use defmt::*;
use defmt_rtt as _;

use embedded_sdmmc::ShortFileName;

use embedded_graphics::{
    mono_font::MonoTextStyle, mono_font::ascii::FONT_5X7, pixelcolor::Rgb565, prelude::*,
    text::Text,
};

use crate::display::FbType;
use crate::input::NavEvent;
use crate::screens::browser::BrowserScreen;
use crate::screens::{Screen, ScreenLogic, Transition};

use crate::dac::client::*;
use crate::sd::DirPath;

pub struct PlayerScreen {
    opened_path: DirPath,
    opened_file: ShortFileName,
}

impl PlayerScreen {
    pub fn new(path: DirPath, file: ShortFileName) -> Self {
        Self {
            opened_file: file,
            opened_path: path,
        }
    }
}

impl ScreenLogic for PlayerScreen {
    async fn on_event(&mut self, event: NavEvent) -> Transition {
        match event {
            NavEvent::Select => {
                info!("[Player] Send DAC request stop signal");
                match end_playback().await {
                    Ok(()) => {}
                    Err(e) => error!("Could not end DAC playback: {}", e),
                }
                Transition::GoTo(Screen::Browser(BrowserScreen::new_with_path(
                    self.opened_path.clone(),
                )))
            }
            _ => Transition::Stay,
        }
    }

    async fn on_enter(&mut self) {
        match start_playback(self.opened_path.clone(), self.opened_file.clone()).await {
            Ok(()) => {}
            Err(e) => error!("Could not start DAC playback: {}", e),
        };
    }

    fn draw(&self, fb: &mut FbType) {
        let style = MonoTextStyle::new(&FONT_5X7, Rgb565::GREEN);

        let filename_text = match core::str::from_utf8(self.opened_file.base_name()) {
            Ok(s) => s,
            Err(_) => "‹invalid utf-8›",
        };

        Text::new(filename_text, Point::new(0, 6), style)
            .draw(fb)
            .unwrap();
    }
}
