// sd/audio.rs
use defmt::*;
use defmt_rtt as _;

use embedded_sdmmc::{RawFile, RawVolume, Volume, VolumeIdx, VolumeManager};

use crate::{
    decoder,
    sd::{
        mp3parse::{Mp3Data, Mp3ParseError, mp3_parse_info, parse_tag_v2},
        wavparse::{WavHeader, parse_wav_header},
    },
};

use super::*;

#[repr(u8)]
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum AudioType {
    WAV = 0,
    MP3 = 1,
}

impl AudioType {
    pub fn from_u8(v: u8) -> Self {
        match v {
            0 => Self::WAV,
            1 => Self::MP3,
            _ => Self::WAV, // fallback for an impossible value
        }
    }
}

/// Stores UI metadata like the idv3 tag
#[derive(Clone)]
pub enum TrackMetadata {
    ID3(Id3v2Tag),
    FileName(ShortFileName), // Wav just stores the filename
}

/// Stores key playback data like the sample rate
#[derive(Clone)]
pub enum AudioInfo {
    MP3(Mp3Data),
    WAV(WavHeader),
}

pub struct AudioPlaybackState {
    pub file: RawFile,
}

pub(super) async fn handle_audio_request<
    'a,
    D,
    T,
    const DIRS: usize,
    const FILES: usize,
    const VOLS: usize,
>(
    request: AudioSdRequest,
    volume_mgr: &'a VolumeManager<D, T, DIRS, FILES, VOLS>,
    playback_state: &mut Option<AudioPlaybackState>,
    volume: &Volume<'a, D, T, DIRS, FILES, VOLS>,
) where
    D: embedded_sdmmc::BlockDevice,
    T: embedded_sdmmc::TimeSource,
{
    match request {
        AudioSdRequest::Open(path, name) => {
            // dosent implment clone i cbb doing it just have 2
            let audio_type = match name.extension() {
                b"WAV" => AudioType::WAV,
                b"MP3" => AudioType::MP3,
                _ => {
                    AUDIO_SD_RESPONSE.signal(AudioSdResponse::Error);
                    return;
                }
            };

            let result = (async || -> Result<(), SdError> {
                let root = volume.open_root_dir().unwrap(); // Handle erors!!!!

                // This is the new working directory
                let dir = open_dir_path(&volume_mgr, root, &path).unwrap();

                let file = dir
                    .open_file_in_dir(name.clone(), embedded_sdmmc::Mode::ReadOnly)
                    .unwrap();

                // Sets TRACK_LOADED, TRACK_METADATA and TRACK_AUDIO_INFO
                let track_metadata: Option<TrackMetadata>;
                let track_info: Option<AudioInfo>;
                match audio_type {
                    AudioType::MP3 => {
                        match parse_tag_v2(&file) {
                            Ok(tag) => {
                                track_metadata = Some(TrackMetadata::ID3(tag));
                            }
                            Err(Mp3ParseError::NoTag) => track_metadata = None,
                            Err(e) => {
                                error!("[SD] MP3 tag parse error {}", Debug2Format(&e));
                                track_metadata = None; // Tag is optional, dosent stop playback
                                // return Err(embedded_sdmmc::Error::EndOfFile);
                            }
                        }
                        match mp3_parse_info(&file) {
                            Ok(info) => {
                                track_info = Some(AudioInfo::MP3(info));
                            }
                            Err(e) => {
                                error!("[SD] MP3 info parse error {}", Debug2Format(&e));
                                return Err(embedded_sdmmc::Error::EndOfFile); // Bad parse prob means MP3 dosent work
                            }
                        }
                    }
                    AudioType::WAV => match parse_wav_header(&file) {
                        Ok(header) => {
                            track_metadata = Some(TrackMetadata::FileName(name.clone()));
                            track_info = Some(AudioInfo::WAV(header));
                        }
                        Err(e) => {
                            error!("[SD] WAV parse error {}", Debug2Format(&e));
                            return Err(embedded_sdmmc::Error::EndOfFile); // Bad parse means no settings give up for now
                        }
                    },
                }
                let sender = TRACK_METADATA.sender();
                sender.send(track_metadata);
                let mut guard = TRACK_AUDIO_INFO.lock().await;
                *guard = track_info; // Authough track info is there, the parser is kinda shit so its not gonna be used
                TRACK_LOADED.store(true, Ordering::Relaxed);
                TRACK_FORMAT.store(audio_type as u8, Ordering::Relaxed);

                // Clear it before if it was accdently left unclosed
                if let Some(mut state) = playback_state.take() {
                    match close(&mut state, volume_mgr) {
                        Ok(()) => {} // yay
                        Err(embedded_sdmmc::Error::BadHandle) => {
                            error!("[SD] Audio open error BadHandle");
                        } // Prob already closed
                        Err(e) => {
                            error!("[SD] Audio open error {}", Debug2Format(&e));
                        } // Otehr error i prob dont care about
                    }
                }

                *playback_state = Some(AudioPlaybackState {
                    file: file.to_raw_file(),
                });
                Ok(())
            })()
            .await;

            // Empty out filled buffer, send to empty buffer
            loop {
                match AUDIO_FILLED.try_receive() {
                    Ok(buf) => AUDIO_EMPTY.send(buf).await,
                    Err(_e) => break,
                }
            }

            match result {
                Ok(()) => match audio_type {
                    AudioType::WAV => {
                        // Fill buffers
                        while let Ok(buf) = AUDIO_EMPTY.try_receive()
                            && let Some(state) = playback_state
                        {
                            match volume_mgr.read(state.file, buf.as_mut_slice()) {
                                Ok(0) => {
                                    AUDIO_EMPTY.send(buf).await;
                                    close(state, &volume_mgr).unwrap();
                                    *playback_state = None;
                                    break;
                                }
                                Ok(n) => {
                                    // Successful read
                                    if n < AUDIO_CHUNK_BYTES {
                                        buf[n..].fill(0);
                                        info!("Trailing zeros in audio chunk, len {}", n);
                                    }
                                    AUDIO_FILLED.send(buf).await; // Hand off data to consumer
                                }
                                Err(_e) => {
                                    AUDIO_EMPTY.send(buf).await;
                                    break;
                                }
                            }
                        }
                        AUDIO_SD_RESPONSE.signal(AudioSdResponse::Opened);
                    }
                    AudioType::MP3 => {
                        // Init and prime decoder etc
                        decoder::DECODER_ACTIVE.store(true, Ordering::Relaxed);
                        AUDIO_SD_RESPONSE.signal(AudioSdResponse::Opened);
                    }
                },
                Err(e) => {
                    error!("[SD] audio open failed: {:?}", Debug2Format(&e));
                    AUDIO_SD_RESPONSE.signal(AudioSdResponse::Error);
                    TRACK_LOADED.store(false, Ordering::Relaxed);
                }
            }
        }
        AudioSdRequest::Close => {
            // Send buffers back
            loop {
                match AUDIO_FILLED.try_receive() {
                    Ok(buf) => AUDIO_EMPTY.send(buf).await,
                    Err(_e) => break,
                }
            }
            TRACK_LOADED.store(false, Ordering::Relaxed);
            if let Some(mut state) = playback_state.take() {
                if let Err(_e) = close(&mut state, volume_mgr) {
                    AUDIO_SD_RESPONSE.signal(AudioSdResponse::Error);
                } else {
                    AUDIO_SD_RESPONSE.signal(AudioSdResponse::Closed);
                }
            } else {
                AUDIO_SD_RESPONSE.signal(AudioSdResponse::Error);
            }
        }
    }
}
