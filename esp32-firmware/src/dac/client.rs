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
        DacResponse::Error(e) => Err(e),
        _ => Err(DacError::UnknownResponse),
    }
}

/// Sends pause signal and awaits response, slow but guaranteed
pub async fn pause() -> Result<(), DacError> {
    DAC_REQUEST.signal(DacRequest::Pause);
    match DAC_RESPONSE.wait().await {
        DacResponse::Paused => Ok(()),
        DacResponse::Error(e) => {
            error!("[DAC] Could not pause: {}", e);
            Err(e)
        }
        _ => Err(DacError::UnknownResponse),
    }
}

/// Sets the paused flag to true, does not wait, instant
pub fn pause_atomic() {
    DAC_PAUSED.store(true, Ordering::Relaxed);
}

/// Sends play signal and awaits response, slow but guaranteed
pub async fn play() -> Result<(), DacError> {
    DAC_REQUEST.signal(DacRequest::Play);
    match DAC_RESPONSE.wait().await {
        DacResponse::Playing => Ok(()),
        DacResponse::Error(e) => {
            error!("[DAC] Could not play: {}", e);
            Err(e)
        }
        _ => Err(DacError::UnknownResponse),
    }
}

/// Sets paused flag to true
pub fn play_atomic() {
    DAC_PAUSED.store(false, Ordering::Relaxed);
}

/// Does the DAC have anything to play (playing or paused)
pub fn has_audio_loaded() -> bool {
    DAC_LOADED.load(Ordering::Relaxed)
}

pub fn paused() -> bool {
    DAC_PAUSED.load(Ordering::Relaxed)
}

pub async fn current_audio() -> Result<(DirPath, ShortFileName, i32), i32> {
    DAC_REQUEST.signal(DacRequest::GetTrackInfo);
    match DAC_RESPONSE.wait().await {
        DacResponse::TrackInfo(info) => match info {
            Ok((path, name, tracknumber)) => Ok((path, name, tracknumber)),
            Err(tracknumber) => Err(tracknumber),
        },
        _ => Err(-1),
    }
}
