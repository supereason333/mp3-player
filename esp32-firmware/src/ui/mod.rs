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

use crate::dac::client::*;
use crate::display::*;
use assets::*;

#[derive(defmt::Format)]
enum Screens {
    Main,
    Browser,
    Queue,
}

#[embassy_executor::task]
pub async fn ui_task(mut display: Display) {
    info!("[UI] UI Task spawned");
    let mut fb: FbType = Framebuffer::new();
    let mut current_screen = Screens::Main;

    match draw_ui_bg(&mut fb, &current_screen) {
        Ok(()) => {}
        Err(e) => error!(
            "Could not draw BG screen '{}' because error '{}'",
            &current_screen,
            Debug2Format(&e)
        ),
    }
    draw_ui_song_info(&mut fb);

    display.write_framebuf(&fb).await;

    loop {}
}

fn draw_ui_song_info(fb: &mut FbType) {
    let (path, filename) = match current_audio() {
        Ok(f) => f,
        Err(()) => return,
    };

    let style = MonoTextStyle::new(&FONT_4X6, Rgb565::WHITE);

    let mut buf: String<16> = String::new(); // 8.3 + dot + null-ish headroom = "XXXXXXXX.XXX" = 12 chars, 16 is safe
    let _ = core::write!(buf, "{}", filename);
    // buf is heapless::String<16>, which derefs to &str:
    let name_str: &str = &buf;

    Text::new(name_str, Point::new(8, 27), style)
        .draw(fb)
        .unwrap();
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
