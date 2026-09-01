use embedded_sdmmc::ShortFileName;

use super::*;
use crate::sd::DirPath;

// Playback control
pub async fn start_playback(path: DirPath, name: ShortFileName) -> Result<(), DacError> {
    DAC_REQUEST.signal(DacRequest::Start(path, name));
    match DAC_RESPONSE.wait().await {
        DacResponse::Opened => Ok(()),
        DacResponse::Error(e) => {
            error!("[DAC] Could not start playback: {}", e);
            Err(e)
        }
        _ => {
            error!("[DAC] Unknown response");
            Err(DacError::UnknownResponse)
        }
    }
}

pub async fn end_playback() -> Result<(), DacError> {
    DAC_REQUEST.signal(DacRequest::Stop);
    match DAC_RESPONSE.wait().await {
        DacResponse::Closed => Ok(()),
        DacResponse::Error(e) => {
            // error!("[DAC] Could not stop playback: {}", e);
            Err(e)
        }
        _ => {
            // error!("[DAC] Unknown response");
            Err(DacError::UnknownResponse)
        }
    }
}

pub fn pause() -> Result<(), ()> {
    error!("[DAC] Pause not implimented");
    Err(())
}

pub fn play() -> Result<(), ()> {
    error!("[DAC] Play not implimented");
    Err(())
}

/// Does the DAC have anything to play
pub fn has_audio_loaded() -> bool {
    false
}

pub fn paused() -> bool {
    false
}

pub fn current_audio() -> Result<(DirPath, ShortFileName), ()> {
    Err(())
}
