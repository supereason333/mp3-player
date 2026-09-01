// A wrapper module for the sd task which takes care of signaling and awaiting and stuff
use super::*;
use embassy_futures::select::{Either, select};

// Audio stuff

/// Opens a file for audio playback. Returns (data_offset, data_size) into the file
/// where PCM data begins.
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

/// Requests the next chunk of audio data. Returns either a filled buffer,
/// or Err(()) on EOF or a read error — check `AUDIO_SD_RESPONSE` semantics
/// via the caller's own EOF handling if you need to distinguish the two.
pub async fn audio_read_chunk() -> Result<AudioChunk, ()> {
    AUDIO_SD_REQUEST.send(AudioSdRequest::ReadChunk).await;

    // A successful read arrives on AUDIO_FILLED; EOF/Error arrive on AUDIO_SD_RESPONSE.
    // Race both since we don't know ahead of time which one will fire.
    match select(AUDIO_FILLED.receive(), AUDIO_SD_RESPONSE.wait()).await {
        Either::First(chunk) => Ok(chunk),
        Either::Second(_response) => Err(()), // Eof or Error, caller can't tell apart currently
    }
}

pub async fn return_audio_chunk(chunk: AudioChunk) -> Result<(), ()> {
    AUDIO_EMPTY.send(chunk).await;
    Ok(())
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
