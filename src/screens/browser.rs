use defmt::*;
use defmt_rtt as _;

use core::fmt::Write;

use embedded_sdmmc::Error::NotFound;
use heapless::String;
use heapless::Vec as HVec;

use embedded_graphics::{
    mono_font::MonoTextStyle, mono_font::ascii::FONT_4X6, pixelcolor::Rgb565, prelude::*,
    text::Text,
};

use crate::display::FbType;
use crate::input::{NAV_EVENT, NavEvent};
use crate::screens::{ScreenLogic, Transition};
use crate::sd::{DirListing, SD_REQUEST, SD_RESPONSE, SdRequest};

pub struct BrowserScreen {
    entries: DirListing,
    selected_index: usize,
    path: HVec<HVec<u8, 12>, 8>,
}

impl BrowserScreen {
    pub fn new() -> Self {
        Self {
            entries: HVec::new(),
            selected_index: 0,
            path: HVec::new(),
        }
    }
}

impl ScreenLogic for BrowserScreen {
    async fn on_enter(&mut self) {
        info!("[Browser] Send SdRequest");
        SD_REQUEST.send(SdRequest::ListDir(HVec::new())).await;
        self.entries = SD_RESPONSE.wait().await;
        info!("[Browser] Recieved Sd Response");
        self.selected_index = 0;
    }

    async fn on_event(&mut self, event: NavEvent) -> Transition {
        match event {
            NavEvent::Up => {
                info!("Up");
                self.selected_index = self.selected_index.saturating_sub(1);
            }
            NavEvent::Down => {
                info!("Down");
                self.selected_index =
                    (self.selected_index + 1).min(self.entries.len().saturating_sub(1));
            }
            NavEvent::Select => {
                info!("Select");
            }
        }
        Transition::Stay
    }

    fn draw(&self, fb: &mut FbType) {
        for (i, (name, _size, _is_dir)) in self.entries.iter().enumerate() {
            let style;
            if i == self.selected_index {
                style = MonoTextStyle::new(&FONT_4X6, Rgb565::GREEN);
            } else {
                style = MonoTextStyle::new(&FONT_4X6, Rgb565::WHITE)
            }

            let mut buf: String<16> = String::new(); // 8.3 + dot + null-ish headroom = "XXXXXXXX.XXX" = 12 chars, 16 is safe
            let _ = core::write!(buf, "{}", name);
            // buf is heapless::String<16>, which derefs to &str:
            let name: &str = &buf;

            Text::new(name, Point::new(0, 8 * i as i32 + 6), style)
                .draw(fb)
                .unwrap();

            // let style = PrimitiveStyleBuilder::new()
            //     .stroke_color(Rgb565::CSS_DARK_SLATE_GRAY)
            //     .stroke_width(3)
            //     .fill_color(Rgb565::CSS_DARK_GRAY)
            //     .build();

            // Rectangle::new(Point::new(1, 1), Size::new(126, 158))
            //     .into_styled(style)
            //     .draw(&mut fb)
            //     .unwrap();
        }
    }
}
