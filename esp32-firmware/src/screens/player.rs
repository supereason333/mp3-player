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

use crate::sd::sd_task::{AUDIO_SD_RESPONSE, AudioSdResponse, DirPath};

use crate::dac::{DAC_REQUEST, DacRequest};

pub struct PlayerScreen {
    opened_path: DirPath,
    opened_file: ShortFileName,
    status: heapless::String<32>,
}

impl PlayerScreen {
    pub fn new(path: DirPath, file: ShortFileName) -> Self {
        Self {
            opened_file: file,
            opened_path: path,
            status: heapless::String::new(),
        }
    }
}

impl ScreenLogic for PlayerScreen {
    async fn on_event(&mut self, event: NavEvent) -> Transition {
        match event {
            NavEvent::Select => {
                info!("[Player] Send DAC request stop signal");
                DAC_REQUEST.signal(DacRequest::Stop);
                loop {
                    match AUDIO_SD_RESPONSE.wait().await {
                        AudioSdResponse::Closed => break,
                        _ => {}
                    }
                }
                Transition::GoTo(Screen::Browser(BrowserScreen::new_with_path(
                    self.opened_path.clone(),
                )))
            }
            _ => Transition::Stay,
        }
    }

    async fn on_enter(&mut self) {
        let _ = self.status.push_str("Opening...");
        DAC_REQUEST.signal(DacRequest::Start(
            self.opened_path.clone(),
            self.opened_file.clone(),
        ))
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
        Text::new(&self.status, Point::new(0, 16), style)
            .draw(fb)
            .unwrap();
    }
}
