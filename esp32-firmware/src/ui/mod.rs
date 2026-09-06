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
    text: String<32>,
    screen_pos: Point,
    space_width: i32,
    scroll_pos: i32,
    t: i32,
}

const SCROLL_INTERVAL: i32 = 1;
const PAUSE_DURATION: i32 = 5;

impl ScrollingText {
    fn new(text: &str, screen_pos: Point, space_width: i32) -> Self {
        let scroll_pos: i32 = if text.len() <= space_width as usize {
            -1
        } else {
            0
        };

        let mut truncated = String::<32>::new();
        let _ = truncated.push_str(&text[..text.len().min(36)]);

        Self {
            text: truncated,
            screen_pos,
            space_width,
            scroll_pos,
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

        let max_scroll = self.text.len() as i32 - self.space_width;
        self.t += dt;

        // Hold longer at the start and end positions, step normally in between
        let at_end = self.scroll_pos == 0 || self.scroll_pos == max_scroll;
        let threshold = if at_end {
            PAUSE_DURATION
        } else {
            SCROLL_INTERVAL
        };

        if self.t >= threshold {
            self.t = 0;
            self.scroll_pos += 1;
            if self.scroll_pos > max_scroll {
                self.scroll_pos = 0;
            }
        }

        let start = self.scroll_pos as usize;
        let end = (self.scroll_pos + self.space_width) as usize;
        let end = end.min(self.text.len());
        let str: &str = self.text.get(start..end).unwrap_or("");
        Text::new(str, self.screen_pos, text_style.clone())
            .draw(fb)
            .unwrap();
    }
    fn set_text(&mut self, text: &str) {
        let mut truncated = String::<32>::new();
        let _ = truncated.push_str(&text[..text.len().min(32)]);
        self.text = truncated;
        self.scroll_pos = if self.text.len() <= self.space_width as usize {
            -1
        } else {
            0
        };
        self.t = 0;
    }

    fn reset_position(&mut self) {
        self.scroll_pos = if self.text.len() <= self.space_width as usize {
            -1
        } else {
            0
        };
        self.t = 0;
    }
}

#[embassy_executor::task]
pub async fn ui_task(mut display: Display) {
    info!("[UI] UI Task spawned");
    let mut fb: FbType = Framebuffer::new();
    let mut current_screen = Screens::Main;

    let mut headertext = ScrollingText::new("Cooper is such a burger", Point::new(3, 7), 8);

    let mut forceupdate = false;

    // Set up main screen struct
    let main_track_text = ScrollingText::new("None", Point::new(10, 27), 10);
    let artist_text = ScrollingText::new("Cooper Burgess", Point::new(10, 39), 10);
    let album_text = ScrollingText::new("Album", Point::new(10, 51), 10);
    let mut main_screen = MainScreen::new(main_track_text, artist_text, album_text);

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
        // let text_style = MonoTextStyle::new(&FONT_4X6, Rgb565::WHITE);
        headertext.update(1, &mut fb);

        let transition = match current_screen {
            Screens::Main => main_screen.update(&mut fb, forceupdate).await,
            Screens::Browser => ScreenTransition::Stay,
            Screens::Queue => ScreenTransition::Stay,
        };
        forceupdate = false;
        display.write_framebuf(&fb).await;

        match transition {
            ScreenTransition::Stay => {}
            _ => {
                match transition {
                    ScreenTransition::Main => current_screen = Screens::Main,
                    ScreenTransition::Browser => current_screen = Screens::Browser,
                    ScreenTransition::Queue => current_screen = Screens::Queue,
                    _ => {}
                }
                forceupdate = true;
            }
        }
        Timer::after(Duration::from_millis(100)).await;
    }
}

struct MainScreen {
    last_track_number: i32,
    main_track_text: ScrollingText,
    artist_text: ScrollingText,
    album_text: ScrollingText,
    updated_on_no_track: bool,
}

impl MainScreen {
    fn new(
        main_track_text: ScrollingText,
        artist_text: ScrollingText,
        album_text: ScrollingText,
    ) -> Self {
        Self {
            main_track_text,
            artist_text,
            album_text,
            last_track_number: -1,
            updated_on_no_track: false,
        }
    }

    async fn update(&mut self, fb: &mut FbType, forceupdate: bool) -> ScreenTransition {
        // let text_style = MonoTextStyle::new(&FONT_4X6, Rgb565::WHITE);

        // Draws audio info
        match current_audio().await {
            Ok((_path, filename, tracknumber)) => {
                if self.last_track_number != tracknumber || forceupdate {
                    self.updated_on_no_track = false;
                    self.last_track_number = tracknumber;
                    let mut buf: String<16> = String::new(); // 8.3 + dot + null-ish headroom = "XXXXXXXX.XXX" = 12 chars, 16 is safe
                    let _ = core::write!(buf, "{}", filename);
                    // buf is heapless::String<16>, which derefs to &str:
                    let name_str: &str = &buf;

                    self.main_track_text.set_text(name_str);
                    self.main_track_text.reset_position();
                }
            }
            Err(_tracknumber) => {
                if !self.updated_on_no_track || forceupdate {
                    self.updated_on_no_track = true;
                    self.main_track_text.set_text("Nothing playing");
                }
            }
        };
        self.main_track_text.update(1, fb);
        self.album_text.update(1, fb);
        self.artist_text.update(1, fb);

        ScreenTransition::Stay
    }
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
