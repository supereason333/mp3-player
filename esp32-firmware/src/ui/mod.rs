pub mod assets;

use defmt::*;
use defmt_rtt as _;

use core::fmt::Write;

use heapless::String;

use embedded_graphics::Drawable;
use embedded_graphics::framebuffer::Framebuffer;
use embedded_graphics::image::Image;
use embedded_graphics::mono_font::MonoTextStyle;
use embedded_graphics::mono_font::ascii::FONT_4X6;
use embedded_graphics::pixelcolor::Rgb565;
use embedded_graphics::prelude::{Point, RgbColor};
use embedded_graphics::text::Text;
use tinybmp::{Bmp, ParseError};

use embassy_time::Duration;
use embassy_time::Timer;

use crate::dac::client::*;
use crate::display::*;
use assets::*;

#[derive(defmt::Format)]
enum Screens {
    Main,
    Browser,
    Queue,
}

enum ScreenTransition {
    Main,
    Browser,
    Queue,
    Stay,
}

struct ScrollingText {
    text: String<16>,
    screen_pos: Point,
    space_width: i32,
    scroll_pos: i32,
    t: i32,
}

impl ScrollingText {
    fn new(text: String<16>, screen_pos: Point, space_width: i32) -> Self {
        // Scroll pos as -1 means text does not need to scroll
        let scroll_pos: i32;
        if text.len() <= space_width as usize {
            scroll_pos = -1;
        } else {
            scroll_pos = 0;
        }
        Self {
            text: text,
            screen_pos: screen_pos,
            space_width: space_width,
            scroll_pos: scroll_pos,
            t: 0,
        }
    }
    fn update(&mut self, dt: i32, fb: &mut FbType) {
        let text_style = MonoTextStyle::new(&FONT_4X6, Rgb565::WHITE);

        if self.scroll_pos == -1 {
            // Dosent need to scroll, just update text
            let name_str: &str = &self.text;
            Text::new(name_str, self.screen_pos, text_style.clone())
                .draw(fb)
                .unwrap();
            return;
        }
        self.scroll_pos += 1;
        if self.scroll_pos > self.space_width - self.text.len() as i32 {
            self.scroll_pos = 0;
        }

        let start = self.scroll_pos as usize;
        let end = (self.scroll_pos + self.space_width) as usize;
        let end = end.min(self.text.len()); // clamp so it never runs past the string

        let str: &str = self.text.get(start..end).unwrap_or("");

        Text::new(str, self.screen_pos, text_style.clone())
            .draw(fb)
            .unwrap();
    }
}

#[embassy_executor::task]
pub async fn ui_task(mut display: Display) {
    info!("[UI] UI Task spawned");
    let mut fb: FbType = Framebuffer::new();
    let mut current_screen = Screens::Main;

    loop {
        match draw_ui_bg(&mut fb, &current_screen) {
            Ok(()) => {}
            Err(e) => error!(
                "Could not draw BG screen '{}' because error '{}'",
                &current_screen,
                Debug2Format(&e)
            ),
        }

        // Draw header info
        let text_style = MonoTextStyle::new(&FONT_4X6, Rgb565::WHITE);

        Text::new(
            "Cooper is such a burger",
            Point::new(3, 7),
            text_style.clone(),
        )
        .draw(&mut fb)
        .unwrap();

        let transition = match current_screen {
            Screens::Main => manage_main_screen(&mut fb),
            Screens::Browser => ScreenTransition::Stay,
            Screens::Queue => ScreenTransition::Stay,
        };
        display.write_framebuf(&fb).await;
        match transition {
            ScreenTransition::Main => current_screen = Screens::Main,
            ScreenTransition::Browser => current_screen = Screens::Browser,
            ScreenTransition::Queue => current_screen = Screens::Queue,
            _ => {}
        }
        Timer::after(Duration::from_secs(1)).await;
    }
}

fn manage_main_screen(fb: &mut FbType) -> ScreenTransition {
    let text_style = MonoTextStyle::new(&FONT_4X6, Rgb565::WHITE);

    // Draws audio info
    match current_audio() {
        Ok((path, filename)) => {
            // Playing so draw text
            let mut buf: String<16> = String::new(); // 8.3 + dot + null-ish headroom = "XXXXXXXX.XXX" = 12 chars, 16 is safe
            let _ = core::write!(buf, "{}", filename);
            // buf is heapless::String<16>, which derefs to &str:
            let name_str: &str = &buf;

            Text::new(name_str, Point::new(8, 27), text_style.clone())
                .draw(fb)
                .unwrap();
        }
        Err(()) => {
            Text::new("Nothing playing", Point::new(8, 27), text_style.clone())
                .draw(fb)
                .unwrap();
        }
    };

    ScreenTransition::Stay
}

fn draw_ui_bg(fb: &mut FbType, screen: &Screens) -> Result<(), ParseError> {
    let bmp = match screen {
        Screens::Main => Bmp::<embedded_graphics::pixelcolor::Rgb565>::from_slice(MAIN_UI_BG),
        Screens::Browser => Bmp::<embedded_graphics::pixelcolor::Rgb565>::from_slice(MAIN_UI_BG),
        Screens::Queue => Bmp::<embedded_graphics::pixelcolor::Rgb565>::from_slice(MAIN_UI_BG),
    }?;
    let image = Image::new(&bmp, Point::zero());
    image.draw(fb).unwrap();
    Ok(())
}
