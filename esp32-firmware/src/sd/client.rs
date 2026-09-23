// sd/client.rs

// A wrapper module for the sd task which takes care of signaling and awaiting and stuff
use super::*;
use embassy_sync::channel::TryReceiveError;

pub enum SdError {
    Generic,
    AudioGeneric,
    AudioEofReached,
    AudioFileNotOpen,
    UiGeneric,
}

// Audio stuff

pub async fn audio_open(path: DirPath, name: ShortFileName) -> Result<(), ()> {
    AUDIO_SD_REQUEST
        .send(AudioSdRequest::Open(path, name))
        .await;
    match AUDIO_SD_RESPONSE.wait().await {
        AudioSdResponse::Opened => {
            info!("[SD] Audio opened");
            Ok(())
        }
        _ => Err(()),
    }
}

pub async fn audio_close() -> Result<(), ()> {
    AUDIO_SD_REQUEST.send(AudioSdRequest::Close).await;
    match AUDIO_SD_RESPONSE.wait().await {
        AudioSdResponse::Closed => {
            info!("[SD] Audio closed");
            Ok(())
        }
        _ => {
            info!("[SD] Audio closed with error");
            Err(())
        }
    }
}

pub fn audio_try_take() -> Result<AudioChunk, TryReceiveError> {
    AUDIO_FILLED.try_receive()
}

pub async fn audio_wait_for_chunk() -> Result<AudioChunk, SdError> {
    if !TRACK_LOADED.load(Ordering::Relaxed) {
        return Err(SdError::AudioFileNotOpen);
    }
    Ok(AUDIO_FILLED.receive().await)
}

pub async fn return_audio_chunk(chunk: AudioChunk) {
    AUDIO_EMPTY.send(chunk).await
}

pub async fn audio_send_filled(chunk: AudioChunk) {
    AUDIO_FILLED.send(chunk).await
}

pub fn audio_try_take_empty() -> Result<AudioChunk, TryReceiveError> {
    AUDIO_EMPTY.try_receive()
}

pub async fn audio_wait_for_empty_chunk() -> AudioChunk {
    AUDIO_EMPTY.receive().await
}

/// Basicaly is track open, if not you prob want to open
pub fn audio_is_track_loaded() -> bool {
    TRACK_LOADED.load(Ordering::Relaxed)
}

// UI stuff

pub async fn ui_list_dir(path: DirPath, offset: usize) -> Result<bool, ()> {
    SD_REQUEST.send(SdRequest::ListDir(path, offset)).await;
    match SD_RESPONSE.wait().await {
        SdResponse::DirLoaded(more) => Ok(more),
        _ => Err(()),
    }
}

pub async fn ui_read_file(path: DirPath, name: ShortFileName) -> Result<HVec<u8, 512>, ()> {
    SD_REQUEST.send(SdRequest::ReadFile(path, name)).await;
    match SD_RESPONSE.wait().await {
        SdResponse::FileContents(data) => Ok(data),
        _ => Err(()),
    }
}

pub async fn get_track_data(path: DirPath, name: ShortFileName) -> Result<ShortFileName, ()> {
    Err(())
}

/// Should only be used by main as a way to see if SD is set up properly, otherwise dont use
pub async fn await_setup_response() -> Result<(), ()> {
    match SD_RESPONSE.wait().await {
        SdResponse::SetupFinished(result) => result,
        _ => Err(()),
    }
}
