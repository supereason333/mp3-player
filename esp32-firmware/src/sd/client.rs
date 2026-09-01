// A wrapper module for the sd task which takes care of signaling and awaiting and stuff
use super::*;
use embassy_futures::select::{Either, select};

pub enum SdError {
    Generic,
    AudioGeneric,
    AudioEofReached,
    AudioFileNotOpen,
    UiGeneric,
}

// Audio stuff

pub async fn audio_open(path: DirPath, name: ShortFileName) -> Result<(u32, u32), ()> {
    AUDIO_SD_REQUEST
        .send(AudioSdRequest::Open(path, name))
        .await;
    match AUDIO_SD_RESPONSE.wait().await {
        AudioSdResponse::Opened {
            data_offset,
            data_size,
        } => Ok((data_offset, data_size)),
        _ => Err(()),
    }
}

pub async fn audio_close() -> Result<(), ()> {
    AUDIO_SD_REQUEST.send(AudioSdRequest::Close).await;
    match AUDIO_SD_RESPONSE.wait().await {
        AudioSdResponse::Closed => Ok(()),
        _ => Err(()),
    }
}

pub async fn audio_read_chunk() -> Result<AudioChunk, SdError> {
    AUDIO_SD_REQUEST.send(AudioSdRequest::ReadChunk).await;

    match select(AUDIO_FILLED.receive(), AUDIO_SD_RESPONSE.wait()).await {
        Either::First(chunk) => Ok(chunk),
        Either::Second(response) => match response {
            AudioSdResponse::Eof => Err(SdError::AudioEofReached),
            AudioSdResponse::Closed => Err(SdError::AudioFileNotOpen),
            AudioSdResponse::Error => Err(SdError::AudioGeneric),
            _ => Err(SdError::Generic),
        },
    }
}

pub async fn return_audio_chunk(chunk: AudioChunk) {
    AUDIO_EMPTY.send(chunk).await;
}

// non blocking varient for getting chunks needed by DAC
pub async fn audio_request_chunk() {
    AUDIO_SD_REQUEST.send(AudioSdRequest::ReadChunk).await;
}

pub fn audio_poll_chunk() -> Option<Result<AudioChunk, ()>> {
    if let Ok(chunk) = AUDIO_FILLED.try_receive() {
        return Some(Ok(chunk));
    }
    if AUDIO_SD_RESPONSE.try_take().is_some() {
        return Some(Err(()));
    }
    None
}

// UI stuff

pub async fn ui_list_dir(path: DirPath) -> Result<DirListing, ()> {
    SD_REQUEST.send(SdRequest::ListDir(path)).await;
    match SD_RESPONSE.wait().await {
        SdResponse::DirListing(listing) => Ok(listing),
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
