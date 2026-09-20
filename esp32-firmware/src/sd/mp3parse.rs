use embedded_sdmmc::File;
use heapless::String as HString;
use nanomp3::{Decoder, MAX_SAMPLES_PER_FRAME};

const MAX_FIELD_LEN: usize = 64;
const PROBE_BUF_LEN: usize = 4096;

#[derive(Debug)]
pub enum Mp3ParseError {
    NoTag,
    FileTooShort,
    ReadError,
    NoFrameFound,
}

#[derive(Debug, Default, Clone)]
pub struct Id3v2Tag {
    pub title: Option<HString<MAX_FIELD_LEN>>,
    pub artist: Option<HString<MAX_FIELD_LEN>>,
    pub album: Option<HString<MAX_FIELD_LEN>>,
    pub year: Option<HString<8>>,
}

#[derive(Clone)]
pub struct Mp3Data {
    pub sample_rate: u32,
}

/// Decode a 28-bit syncsafe integer (7 usable bits per byte, MSB always 0).
/// Used for the overall tag size in every ID3v2 version, and for individual
/// frame sizes in v2.4 only — v2.2/v2.3 frame sizes are plain big-endian.
fn syncsafe(bytes: [u8; 4]) -> u32 {
    ((bytes[0] as u32 & 0x7f) << 21)
        | ((bytes[1] as u32 & 0x7f) << 14)
        | ((bytes[2] as u32 & 0x7f) << 7)
        | (bytes[3] as u32 & 0x7f)
}

/// Decode an ID3v2 text frame's content (encoding byte + text) into a
/// heapless String. Latin-1 and UTF-8 are decoded fully; UTF-16 is decoded
/// best-effort on the BMP (no surrogate-pair handling) since embedded
/// titles/artists/albums are overwhelmingly within that range in practice.
fn decode_text<const N: usize>(data: &[u8]) -> Option<HString<N>> {
    if data.is_empty() {
        return None;
    }
    let (encoding, body) = (data[0], &data[1..]);
    let mut out: HString<N> = HString::new();

    match encoding {
        0 => {
            // ISO-8859-1: byte value == Unicode codepoint for 0..=255.
            for &b in body {
                if b == 0 {
                    break;
                }
                if out.push(b as char).is_err() {
                    break; // longer than our buffer — truncate silently
                }
            }
        }
        3 => {
            // UTF-8, null-terminated.
            let end = body.iter().position(|&b| b == 0).unwrap_or(body.len());
            if let Ok(s) = core::str::from_utf8(&body[..end]) {
                for c in s.chars() {
                    if out.push(c).is_err() {
                        break;
                    }
                }
            }
        }
        1 | 2 => {
            // UTF-16: encoding 1 has a BOM (FFFE/FEFF) determining endianness,
            // encoding 2 is always big-endian with no BOM.
            let mut big_endian = encoding == 2;
            let mut first_pair = true;
            for pair in body.chunks_exact(2) {
                let code = if big_endian {
                    u16::from_be_bytes([pair[0], pair[1]])
                } else {
                    u16::from_le_bytes([pair[0], pair[1]])
                };
                if first_pair && encoding == 1 {
                    first_pair = false;
                    match code {
                        0xFEFF => continue, // BOM confirms LE, already assumed
                        0xFFFE => {
                            big_endian = true;
                            continue;
                        }
                        _ => {}
                    }
                }
                if code == 0 {
                    break;
                }
                if (0xD800..=0xDFFF).contains(&code) {
                    break; // surrogate pair — not handled, stop here
                }
                if let Some(c) = char::from_u32(code as u32) {
                    if out.push(c).is_err() {
                        break;
                    }
                }
            }
        }
        _ => return None,
    }

    if out.is_empty() { None } else { Some(out) }
}

/// Reads and discards `count` bytes forward in the file, since we only need
/// sequential access here (no seek call used elsewhere in this codebase).
fn skip_bytes<D, T, const DIRS: usize, const FILES: usize, const VOLS: usize>(
    file: &File<D, T, DIRS, FILES, VOLS>,
    mut count: u32,
) -> Result<(), Mp3ParseError>
where
    D: embedded_sdmmc::BlockDevice,
    T: embedded_sdmmc::TimeSource,
{
    let mut scratch = [0u8; 64];
    while count > 0 {
        let want = (count as usize).min(scratch.len());
        let n = file
            .read(&mut scratch[..want])
            .map_err(|_| Mp3ParseError::ReadError)?;
        if n == 0 {
            return Err(Mp3ParseError::FileTooShort);
        }
        count -= n as u32;
    }
    Ok(())
}

/// Parses the ID3v2 tag at the start of the MP3 file, if present.
/// Only Title, Artist, Album, and Year are extracted; all other frames are
/// skipped. Supports ID3v2.2 (3-char frame IDs), ID3v2.3, and ID3v2.4
/// (4-char frame IDs; year comes from TYER in v2.3 or TDRC in v2.4).
pub fn parse_tag_v2<D, T, const DIRS: usize, const FILES: usize, const VOLS: usize>(
    file: &File<D, T, DIRS, FILES, VOLS>,
) -> Result<Id3v2Tag, Mp3ParseError>
where
    D: embedded_sdmmc::BlockDevice,
    T: embedded_sdmmc::TimeSource,
{
    // Full 10-byte header: "ID3" + version(2) + flags(1) + syncsafe size(4).
    let mut header = [0u8; 10];
    let n = file
        .read(&mut header)
        .map_err(|_| Mp3ParseError::ReadError)?;
    if n != 10 {
        return Err(Mp3ParseError::FileTooShort);
    }
    if &header[0..3] != b"ID3" {
        return Err(Mp3ParseError::NoTag);
    }

    let major_version = header[3]; // 2, 3, or 4
    let flags = header[5];
    let tag_size = syncsafe([header[6], header[7], header[8], header[9]]);
    let audio_start = 10u32 + tag_size; // absolute offset

    // Unsynchronisation (bit 7) rewrites frame bytes in a way this parser
    // doesn't reverse — bail rather than return corrupted text.
    if flags & 0x80 != 0 {
        return Err(Mp3ParseError::NoTag);
    }
    // Extended header (bit 6, v2.3+) — not handled here.
    if major_version >= 3 && flags & 0x40 != 0 {
        return Err(Mp3ParseError::NoTag);
    }

    let mut tag = Id3v2Tag::default();
    let mut remaining = tag_size;

    let id_len: usize = if major_version == 2 { 3 } else { 4 };
    let size_len: usize = if major_version == 2 { 3 } else { 4 };
    let flags_len: usize = if major_version == 2 { 0 } else { 2 };
    let frame_header_len = id_len + size_len + flags_len;

    let mut frame_header = [0u8; 10];
    let mut scratch = [0u8; MAX_FIELD_LEN + 8];

    while remaining >= frame_header_len as u32 {
        let n = file
            .read(&mut frame_header[..frame_header_len])
            .map_err(|_| Mp3ParseError::ReadError)?;
        if n != frame_header_len {
            break; // truncated tag
        }
        remaining -= frame_header_len as u32;

        let id = &frame_header[..id_len];
        if id.iter().all(|&b| b == 0) {
            break; // padding reached, no more frames
        }

        let frame_size = if major_version == 2 {
            ((frame_header[3] as u32) << 16)
                | ((frame_header[4] as u32) << 8)
                | (frame_header[5] as u32)
        } else if major_version == 4 {
            syncsafe([
                frame_header[4],
                frame_header[5],
                frame_header[6],
                frame_header[7],
            ])
        } else {
            // v2.3: plain big-endian, NOT syncsafe.
            u32::from_be_bytes([
                frame_header[4],
                frame_header[5],
                frame_header[6],
                frame_header[7],
            ])
        };

        if frame_size > remaining {
            break; // malformed size — stop rather than read past tag bounds
        }
        remaining -= frame_size;

        let is_title = id == b"TT2" || id == b"TIT2";
        let is_artist = id == b"TP1" || id == b"TPE1";
        let is_album = id == b"TAL" || id == b"TALB";
        let is_year = id == b"TYE" || id == b"TYER" || id == b"TDRC";
        let wanted = is_title || is_artist || is_album || is_year;

        if wanted && frame_size > 0 {
            let read_len = (frame_size as usize).min(scratch.len());
            let n = file
                .read(&mut scratch[..read_len])
                .map_err(|_| Mp3ParseError::ReadError)?;
            if n != read_len {
                break;
            }
            if (frame_size as usize) > read_len {
                skip_bytes(file, frame_size - read_len as u32)?;
            }

            if is_title && tag.title.is_none() {
                tag.title = decode_text(&scratch[..read_len]);
            } else if is_artist && tag.artist.is_none() {
                tag.artist = decode_text(&scratch[..read_len]);
            } else if is_album && tag.album.is_none() {
                tag.album = decode_text(&scratch[..read_len]);
            } else if is_year && tag.year.is_none() {
                if let Some(full) = decode_text::<16>(&scratch[..read_len]) {
                    // TDRC (v2.4) can hold a full date, e.g. "2024-05-01" —
                    // keep just the first 4 characters as the year.
                    let mut y: HString<8> = HString::new();
                    for c in full.chars().take(4) {
                        let _ = y.push(c);
                    }
                    tag.year = Some(y);
                }
            }
        } else {
            skip_bytes(file, frame_size)?;
        }
    }

    file.seek_from_start(audio_start)
        .map_err(|_| Mp3ParseError::ReadError)?;

    Ok(tag)
}

pub fn mp3_parse_info<D, T, const DIRS: usize, const FILES: usize, const VOLS: usize>(
    file: &File<D, T, DIRS, FILES, VOLS>,
) -> Result<Mp3Data, Mp3ParseError>
where
    D: embedded_sdmmc::BlockDevice,
    T: embedded_sdmmc::TimeSource,
{
    // Remember exactly where the first frame starts, to seek back to later.
    let frame_start = file.offset();

    let mut probe = [0u8; PROBE_BUF_LEN];
    let n = file
        .read(&mut probe)
        .map_err(|_| Mp3ParseError::ReadError)?;

    // let mut decoder = Decoder::new();
    // let mut pcm_scratch = [0f32; MAX_SAMPLES_PER_FRAME];
    // let (_consumed, frame_info) = decoder.decode(&probe[..n], &mut pcm_scratch);

    // Seek back regardless of outcome, so the file cursor ends up in a
    // known/predictable place either way rather than wherever decode()
    // happened to leave the probe read.
    file.seek_from_start(frame_start)
        .map_err(|_| Mp3ParseError::ReadError)?;

    // let info = frame_info.ok_or(Mp3ParseError::NoFrameFound)?;

    Ok(Mp3Data {
        // sample_rate: info.sample_rate,
        sample_rate: 0,
    })
}
