//! WAV decoding.
//!
//! The capture path never sees a WAV file — WASAPI hands over raw PCM — but
//! everything used to *develop and test* the audio path does. Without a decoder
//! the only way to exercise the VAD, the resampler, and the speech-to-text
//! adapter is to synthesise tones, and a VAD tuned against sine waves is a VAD
//! tuned against nothing.
//!
//! So this exists to make real recorded speech usable: read a file, downmix it,
//! resample it to the wire rate, and feed it to the same pipeline that captured
//! audio will use.
//!
//! # Scope
//!
//! Uncompressed PCM and IEEE float, in the bit depths Windows and every
//! recorder actually produce. Compressed containers (MP3, Opus, AAC) are
//! **refused by name** rather than misparsed — a decoder that silently returns
//! noise is worse than one that says it cannot read the file.
//!
//! No dependency: RIFF is a chunk list, and the whole format is a few hundred
//! lines. Pulling in an audio library to read a header would be the wrong trade
//! for a client whose selling point is being small.

use std::fmt;

/// How the samples are encoded.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SampleFormat {
    /// Unsigned 8-bit PCM.
    U8,
    /// Signed 16-bit little-endian PCM.
    I16,
    /// Signed 24-bit little-endian PCM.
    I24,
    /// Signed 32-bit little-endian PCM.
    I32,
    /// 32-bit IEEE float.
    F32,
}

impl SampleFormat {
    /// Bytes per sample.
    pub const fn bytes(self) -> usize {
        match self {
            SampleFormat::U8 => 1,
            SampleFormat::I16 => 2,
            SampleFormat::I24 => 3,
            SampleFormat::I32 => 4,
            SampleFormat::F32 => 4,
        }
    }

    /// A short label for diagnostics.
    pub const fn label(self) -> &'static str {
        match self {
            SampleFormat::U8 => "u8",
            SampleFormat::I16 => "i16",
            SampleFormat::I24 => "i24",
            SampleFormat::I32 => "i32",
            SampleFormat::F32 => "f32",
        }
    }
}

/// Decoded audio, still interleaved.
#[derive(Debug, Clone, PartialEq)]
pub struct WavAudio {
    /// Sample rate in Hz.
    pub sample_rate_hz: u32,
    /// Interleaved channel count.
    pub channels: u16,
    /// How the samples were encoded in the file.
    pub format: SampleFormat,
    /// Interleaved samples, normalised to `i16`.
    pub samples: Vec<i16>,
}

impl WavAudio {
    /// Frames (one sample per channel).
    pub fn frame_count(&self) -> usize {
        if self.channels == 0 {
            0
        } else {
            self.samples.len() / self.channels as usize
        }
    }

    /// Duration.
    pub fn duration(&self) -> std::time::Duration {
        if self.sample_rate_hz == 0 {
            return std::time::Duration::ZERO;
        }
        std::time::Duration::from_secs_f64(
            self.frame_count() as f64 / f64::from(self.sample_rate_hz),
        )
    }

    /// Average the channels down to mono.
    ///
    /// Averaging rather than taking the first channel: game audio is frequently
    /// mixed so that speech sits off-centre, and dropping a channel would drop
    /// part of the callout.
    pub fn to_mono(&self) -> Vec<i16> {
        let channels = self.channels.max(1) as usize;
        if channels == 1 {
            return self.samples.clone();
        }

        self.samples
            .chunks_exact(channels)
            .map(|frame| {
                let sum: i32 = frame.iter().map(|&s| i32::from(s)).sum();
                (sum / channels as i32).clamp(i32::from(i16::MIN), i32::from(i16::MAX)) as i16
            })
            .collect()
    }
}

/// Why a file could not be decoded.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum WavError {
    /// The file is shorter than the smallest valid header.
    Truncated {
        /// What was being read.
        what: String,
    },
    /// The file is not RIFF/WAVE at all.
    NotWav,
    /// The file is a compressed container this decoder does not read.
    UnsupportedContainer {
        /// The format name, when it could be identified.
        format: String,
    },
    /// The `fmt ` chunk declares an encoding this decoder does not read.
    UnsupportedEncoding {
        /// The numeric format code from the file.
        code: u16,
        /// Bits per sample declared.
        bits: u16,
    },
    /// No `fmt ` chunk was found.
    MissingFormatChunk,
    /// No `data` chunk was found.
    MissingDataChunk,
    /// The `fmt ` chunk is internally inconsistent.
    MalformedFormat {
        /// What was wrong.
        detail: String,
    },
}

impl fmt::Display for WavError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            WavError::Truncated { what } => write!(f, "the file ended while reading {what}"),
            WavError::NotWav => write!(f, "this is not a WAV file"),
            WavError::UnsupportedContainer { format } => write!(
                f,
                "{format} is a compressed format; convert it to WAV first"
            ),
            WavError::UnsupportedEncoding { code, bits } => write!(
                f,
                "unsupported WAV encoding (format code {code}, {bits} bits per sample)"
            ),
            WavError::MissingFormatChunk => write!(f, "the WAV file has no format chunk"),
            WavError::MissingDataChunk => write!(f, "the WAV file has no audio data"),
            WavError::MalformedFormat { detail } => write!(f, "malformed WAV format chunk: {detail}"),
        }
    }
}

impl std::error::Error for WavError {}

/// Format codes from the WAVE specification.
const FORMAT_PCM: u16 = 0x0001;
const FORMAT_IEEE_FLOAT: u16 = 0x0003;
/// `WAVE_FORMAT_EXTENSIBLE`, which carries the real format in a sub-format GUID.
const FORMAT_EXTENSIBLE: u16 = 0xFFFE;

/// Decode a WAV file.
pub fn decode(bytes: &[u8]) -> Result<WavAudio, WavError> {
    // Identify compressed containers by their magic so the error names the
    // format. A user who pointed the client at an MP3 deserves to be told that,
    // not handed "not a WAV file".
    if let Some(format) = sniff_compressed(bytes) {
        return Err(WavError::UnsupportedContainer {
            format: format.to_string(),
        });
    }

    if bytes.len() < 12 {
        return Err(WavError::Truncated {
            what: "the RIFF header".to_string(),
        });
    }
    if &bytes[0..4] != b"RIFF" || &bytes[8..12] != b"WAVE" {
        return Err(WavError::NotWav);
    }

    let mut format: Option<FormatChunk> = None;
    let mut data: Option<&[u8]> = None;

    // Walk the chunk list from after the RIFF header.
    let mut position = 12usize;
    while position + 8 <= bytes.len() {
        let id = &bytes[position..position + 4];
        let declared = u32::from_le_bytes([
            bytes[position + 4],
            bytes[position + 5],
            bytes[position + 6],
            bytes[position + 7],
        ]) as usize;
        let body_start = position + 8;

        // A chunk whose declared size runs past the end is treated as running
        // to the end. Some writers, and some pipes, leave the last size wrong.
        let body_end = body_start.saturating_add(declared).min(bytes.len());
        let body = &bytes[body_start..body_end];

        match id {
            b"fmt " => format = Some(parse_format_chunk(body)?),
            b"data" => {
                data = Some(body);
                // `data` is conventionally last, but the loop continues so a
                // trailing chunk does not cause the data to be discarded.
            }
            _ => {}
        }

        // Chunks are word-aligned: an odd size is followed by a pad byte.
        let advance = 8 + declared + (declared % 2);
        if advance == 0 {
            break;
        }
        position = position.saturating_add(advance);
    }

    let format = format.ok_or(WavError::MissingFormatChunk)?;
    let data = data.ok_or(WavError::MissingDataChunk)?;

    let samples = convert_samples(data, format)?;

    Ok(WavAudio {
        sample_rate_hz: format.sample_rate_hz,
        channels: format.channels,
        format: format.sample_format,
        samples,
    })
}

/// Identify a compressed container so the error can name it.
fn sniff_compressed(bytes: &[u8]) -> Option<&'static str> {
    if bytes.len() < 12 {
        return None;
    }
    // MP3: an ID3 tag, or a frame sync.
    if &bytes[0..3] == b"ID3" {
        return Some("MP3");
    }
    if bytes[0] == 0xFF && (bytes[1] & 0xE0) == 0xE0 {
        return Some("MP3");
    }
    // Ogg, which is usually Opus or Vorbis.
    if &bytes[0..4] == b"OggS" {
        return Some("Ogg (Opus or Vorbis)");
    }
    // MP4/M4A/AAC: an `ftyp` box.
    if bytes.len() >= 12 && &bytes[4..8] == b"ftyp" {
        return Some("MP4/M4A/AAC");
    }
    // FLAC.
    if &bytes[0..4] == b"fLaC" {
        return Some("FLAC");
    }
    // Matroska/WebM.
    if bytes.len() >= 4 && &bytes[0..4] == b"\x1A\x45\xDF\xA3" {
        return Some("Matroska/WebM");
    }
    None
}

/// The parsed `fmt ` chunk.
///
/// `Copy` because it is a handful of scalars and the decode path reads it after
/// the samples have been converted.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
struct FormatChunk {
    sample_format: SampleFormat,
    channels: u16,
    sample_rate_hz: u32,
}

fn parse_format_chunk(body: &[u8]) -> Result<FormatChunk, WavError> {
    if body.len() < 16 {
        return Err(WavError::Truncated {
            what: "the format chunk".to_string(),
        });
    }

    let mut code = u16::from_le_bytes([body[0], body[1]]);
    let channels = u16::from_le_bytes([body[2], body[3]]);
    let sample_rate_hz = u32::from_le_bytes([body[4], body[5], body[6], body[7]]);
    let bits = u16::from_le_bytes([body[14], body[15]]);

    // WAVE_FORMAT_EXTENSIBLE hides the real format in the first two bytes of
    // the sub-format GUID, which sits at offset 24.
    if code == FORMAT_EXTENSIBLE {
        if body.len() < 26 {
            return Err(WavError::MalformedFormat {
                detail: "extensible format chunk is too short to carry a sub-format".to_string(),
            });
        }
        code = u16::from_le_bytes([body[24], body[25]]);
    }

    if channels == 0 {
        return Err(WavError::MalformedFormat {
            detail: "zero channels".to_string(),
        });
    }
    if sample_rate_hz == 0 {
        return Err(WavError::MalformedFormat {
            detail: "zero sample rate".to_string(),
        });
    }

    let sample_format = match (code, bits) {
        (FORMAT_PCM, 8) => SampleFormat::U8,
        (FORMAT_PCM, 16) => SampleFormat::I16,
        (FORMAT_PCM, 24) => SampleFormat::I24,
        (FORMAT_PCM, 32) => SampleFormat::I32,
        (FORMAT_IEEE_FLOAT, 32) => SampleFormat::F32,
        _ => {
            return Err(WavError::UnsupportedEncoding { code, bits });
        }
    };

    Ok(FormatChunk {
        sample_format,
        channels,
        sample_rate_hz,
    })
}

/// Convert raw sample bytes into `i16`.
fn convert_samples(data: &[u8], format: FormatChunk) -> Result<Vec<i16>, WavError> {
    let width = format.sample_format.bytes();
    let count = data.len() / width;
    let mut out = Vec::with_capacity(count);

    for index in 0..count {
        let start = index * width;
        let bytes = &data[start..start + width];
        out.push(match format.sample_format {
            // 8-bit WAV is unsigned, centred on 128. Treating it as signed is a
            // classic bug that turns quiet passages into full-scale noise.
            SampleFormat::U8 => (i16::from(bytes[0]) - 128) * 256,
            SampleFormat::I16 => i16::from_le_bytes([bytes[0], bytes[1]]),
            SampleFormat::I24 => {
                // Sign-extend the 24-bit value into 32 bits, then take the top
                // 16 so full scale stays full scale.
                let raw = i32::from_le_bytes([bytes[0], bytes[1], bytes[2], 0]);
                let signed = (raw << 8) >> 8;
                (signed >> 8) as i16
            }
            SampleFormat::I32 => {
                let raw = i32::from_le_bytes([bytes[0], bytes[1], bytes[2], bytes[3]]);
                (raw >> 16) as i16
            }
            SampleFormat::F32 => {
                let raw = f32::from_le_bytes([bytes[0], bytes[1], bytes[2], bytes[3]]);
                // Clamp before scaling: a float sample outside [-1, 1] would
                // otherwise wrap when cast.
                (raw.clamp(-1.0, 1.0) * 32767.0) as i16
            }
        });
    }

    Ok(out)
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Build a minimal WAV file around raw sample bytes.
    fn wav(
        format_code: u16,
        channels: u16,
        sample_rate: u32,
        bits: u16,
        data: &[u8],
    ) -> Vec<u8> {
        let mut fmt = Vec::new();
        fmt.extend_from_slice(&format_code.to_le_bytes());
        fmt.extend_from_slice(&channels.to_le_bytes());
        fmt.extend_from_slice(&sample_rate.to_le_bytes());
        let block_align = channels * (bits / 8);
        fmt.extend_from_slice(&(sample_rate * u32::from(block_align)).to_le_bytes());
        fmt.extend_from_slice(&block_align.to_le_bytes());
        fmt.extend_from_slice(&bits.to_le_bytes());

        let riff_size = (4 + 8 + fmt.len() + 8 + data.len()) as u32;
        let mut out = Vec::new();
        out.extend_from_slice(b"RIFF");
        out.extend_from_slice(&riff_size.to_le_bytes());
        out.extend_from_slice(b"WAVE");
        out.extend_from_slice(b"fmt ");
        out.extend_from_slice(&(fmt.len() as u32).to_le_bytes());
        out.extend_from_slice(&fmt);
        out.extend_from_slice(b"data");
        out.extend_from_slice(&(data.len() as u32).to_le_bytes());
        out.extend_from_slice(data);
        out
    }

    fn i16_bytes(samples: &[i16]) -> Vec<u8> {
        samples.iter().flat_map(|s| s.to_le_bytes()).collect()
    }

    #[test]
    fn a_mono_16_bit_file_decodes() {
        let samples = [0i16, 1000, -1000, i16::MAX, i16::MIN];
        let file = wav(FORMAT_PCM, 1, 16_000, 16, &i16_bytes(&samples));
        let audio = decode(&file).expect("should decode");

        assert_eq!(audio.sample_rate_hz, 16_000);
        assert_eq!(audio.channels, 1);
        assert_eq!(audio.format, SampleFormat::I16);
        assert_eq!(audio.samples, samples);
    }

    #[test]
    fn duration_is_derived_from_the_sample_rate() {
        let samples = vec![0i16; 16_000];
        let file = wav(FORMAT_PCM, 1, 16_000, 16, &i16_bytes(&samples));
        let audio = decode(&file).unwrap();
        assert_eq!(audio.frame_count(), 16_000);
        assert_eq!(audio.duration().as_secs_f32(), 1.0);
    }

    #[test]
    fn a_stereo_file_reports_its_channel_count() {
        // Interleaved L R L R.
        let samples = [100i16, 200, 300, 400];
        let file = wav(FORMAT_PCM, 2, 44_100, 16, &i16_bytes(&samples));
        let audio = decode(&file).unwrap();
        assert_eq!(audio.channels, 2);
        assert_eq!(audio.frame_count(), 2);
        assert_eq!(audio.samples, samples);
    }

    #[test]
    fn stereo_downmixes_by_averaging_not_by_dropping_a_channel() {
        // Game audio often puts speech off-centre; dropping a channel would
        // drop part of the callout.
        let samples = [1000i16, 3000, -1000, -3000];
        let file = wav(FORMAT_PCM, 2, 16_000, 16, &i16_bytes(&samples));
        let audio = decode(&file).unwrap();

        let mono = audio.to_mono();
        assert_eq!(mono, vec![2000i16, -2000]);
    }

    #[test]
    fn mono_downmix_is_a_passthrough() {
        let samples = [1i16, 2, 3];
        let file = wav(FORMAT_PCM, 1, 16_000, 16, &i16_bytes(&samples));
        assert_eq!(decode(&file).unwrap().to_mono(), samples.to_vec());
    }

    #[test]
    fn downmixing_does_not_overflow() {
        // Summing two full-scale samples in i16 would wrap.
        let samples = [i16::MAX, i16::MAX];
        let file = wav(FORMAT_PCM, 2, 16_000, 16, &i16_bytes(&samples));
        let mono = decode(&file).unwrap().to_mono();
        assert_eq!(mono, vec![i16::MAX]);
    }

    #[test]
    fn eight_bit_pcm_is_unsigned_and_centred_on_128() {
        // The classic 8-bit WAV bug: treating the samples as signed turns a
        // quiet passage into full-scale noise.
        let data = [128u8, 129, 127, 255, 0];
        let file = wav(FORMAT_PCM, 1, 8_000, 8, &data);
        let audio = decode(&file).unwrap();

        assert_eq!(audio.format, SampleFormat::U8);
        assert_eq!(audio.samples[0], 0, "128 is silence");
        assert_eq!(audio.samples[1], 256);
        assert_eq!(audio.samples[2], -256);
        assert!(audio.samples[3] > 30_000, "255 is near full scale");
        assert!(audio.samples[4] < -30_000, "0 is near negative full scale");
    }

    #[test]
    fn twenty_four_bit_pcm_sign_extends_correctly() {
        // 0x000000 is silence, 0x7FFFFF is full positive, 0x800000 full negative.
        let data = [0x00, 0x00, 0x00, 0xFF, 0xFF, 0x7F, 0x00, 0x00, 0x80];
        let file = wav(FORMAT_PCM, 1, 16_000, 24, &data);
        let audio = decode(&file).unwrap();

        assert_eq!(audio.format, SampleFormat::I24);
        assert_eq!(audio.samples[0], 0);
        assert!(audio.samples[1] > 32_000, "positive full scale");
        assert!(audio.samples[2] < -32_000, "negative full scale");
    }

    #[test]
    fn thirty_two_bit_pcm_takes_the_high_word() {
        let data = [
            0x00, 0x00, 0x00, 0x00, // 0
            0x00, 0x00, 0x00, 0x40, // large positive
            0x00, 0x00, 0x00, 0xC0, // large negative
        ];
        let file = wav(FORMAT_PCM, 1, 16_000, 32, &data);
        let audio = decode(&file).unwrap();

        assert_eq!(audio.format, SampleFormat::I32);
        assert_eq!(audio.samples[0], 0);
        assert!(audio.samples[1] > 0);
        assert!(audio.samples[2] < 0);
    }

    #[test]
    fn thirty_two_bit_float_decodes_and_clamps() {
        let mut data = Vec::new();
        for value in [0.0f32, 0.5, -0.5, 1.0, -1.0, 5.0, -5.0] {
            data.extend_from_slice(&value.to_le_bytes());
        }
        let file = wav(FORMAT_IEEE_FLOAT, 1, 16_000, 32, &data);
        let audio = decode(&file).unwrap();

        assert_eq!(audio.format, SampleFormat::F32);
        assert_eq!(audio.samples[0], 0);
        assert!((i32::from(audio.samples[1]) - 16383).abs() < 10);
        assert!((i32::from(audio.samples[2]) + 16383).abs() < 10);
        // Out-of-range floats must clamp, not wrap.
        assert!(audio.samples[5] > 30_000);
        assert!(audio.samples[6] < -30_000);
    }

    #[test]
    fn an_extensible_header_resolves_its_subformat() {
        // WAVE_FORMAT_EXTENSIBLE: the real format is in the sub-format GUID.
        let mut fmt = Vec::new();
        fmt.extend_from_slice(&FORMAT_EXTENSIBLE.to_le_bytes());
        fmt.extend_from_slice(&1u16.to_le_bytes()); // mono
        fmt.extend_from_slice(&16_000u32.to_le_bytes());
        fmt.extend_from_slice(&32_000u32.to_le_bytes());
        fmt.extend_from_slice(&2u16.to_le_bytes());
        fmt.extend_from_slice(&16u16.to_le_bytes());
        fmt.extend_from_slice(&22u16.to_le_bytes()); // cbSize
        fmt.extend_from_slice(&16u16.to_le_bytes()); // valid bits
        fmt.extend_from_slice(&3u32.to_le_bytes()); // channel mask
        fmt.extend_from_slice(&FORMAT_PCM.to_le_bytes());
        fmt.extend_from_slice(&[0u8; 14]); // rest of the GUID

        let samples = [1234i16, -1234];
        let data = i16_bytes(&samples);

        let riff_size = (4 + 8 + fmt.len() + 8 + data.len()) as u32;
        let mut out = Vec::new();
        out.extend_from_slice(b"RIFF");
        out.extend_from_slice(&riff_size.to_le_bytes());
        out.extend_from_slice(b"WAVE");
        out.extend_from_slice(b"fmt ");
        out.extend_from_slice(&(fmt.len() as u32).to_le_bytes());
        out.extend_from_slice(&fmt);
        out.extend_from_slice(b"data");
        out.extend_from_slice(&(data.len() as u32).to_le_bytes());
        out.extend_from_slice(&data);

        let audio = decode(&out).expect("extensible PCM should decode");
        assert_eq!(audio.samples, samples);
        assert_eq!(audio.format, SampleFormat::I16);
    }

    #[test]
    fn unknown_chunks_are_skipped() {
        // Real files carry LIST, fact, and bext chunks; a decoder that assumed
        // `fmt ` was first would fail on most of them.
        let samples = [7i16, -7];
        let data = i16_bytes(&samples);
        let mut fmt = Vec::new();
        fmt.extend_from_slice(&FORMAT_PCM.to_le_bytes());
        fmt.extend_from_slice(&1u16.to_le_bytes());
        fmt.extend_from_slice(&16_000u32.to_le_bytes());
        fmt.extend_from_slice(&32_000u32.to_le_bytes());
        fmt.extend_from_slice(&2u16.to_le_bytes());
        fmt.extend_from_slice(&16u16.to_le_bytes());

        let mut out = Vec::new();
        out.extend_from_slice(b"RIFF");
        out.extend_from_slice(&0u32.to_le_bytes());
        out.extend_from_slice(b"WAVE");
        // A LIST chunk before fmt.
        out.extend_from_slice(b"LIST");
        out.extend_from_slice(&4u32.to_le_bytes());
        out.extend_from_slice(b"INFO");
        out.extend_from_slice(b"fmt ");
        out.extend_from_slice(&(fmt.len() as u32).to_le_bytes());
        out.extend_from_slice(&fmt);
        out.extend_from_slice(b"data");
        out.extend_from_slice(&(data.len() as u32).to_le_bytes());
        out.extend_from_slice(&data);

        let audio = decode(&out).expect("should skip unknown chunks");
        assert_eq!(audio.samples, samples);
    }

    #[test]
    fn an_odd_sized_chunk_is_padded_correctly() {
        // A chunk with an odd size is followed by a pad byte. Ignoring the pad
        // shifts every later chunk by one and corrupts the parse.
        let samples = [42i16];
        let data = i16_bytes(&samples);
        let mut fmt = Vec::new();
        fmt.extend_from_slice(&FORMAT_PCM.to_le_bytes());
        fmt.extend_from_slice(&1u16.to_le_bytes());
        fmt.extend_from_slice(&16_000u32.to_le_bytes());
        fmt.extend_from_slice(&32_000u32.to_le_bytes());
        fmt.extend_from_slice(&2u16.to_le_bytes());
        fmt.extend_from_slice(&16u16.to_le_bytes());

        let mut out = Vec::new();
        out.extend_from_slice(b"RIFF");
        out.extend_from_slice(&0u32.to_le_bytes());
        out.extend_from_slice(b"WAVE");
        out.extend_from_slice(b"fmt ");
        out.extend_from_slice(&(fmt.len() as u32).to_le_bytes());
        out.extend_from_slice(&fmt);
        // An odd-sized chunk: 3 bytes of payload plus a pad byte.
        out.extend_from_slice(b"junk");
        out.extend_from_slice(&3u32.to_le_bytes());
        out.extend_from_slice(&[1, 2, 3, 0]);
        out.extend_from_slice(b"data");
        out.extend_from_slice(&(data.len() as u32).to_le_bytes());
        out.extend_from_slice(&data);

        let audio = decode(&out).expect("should handle the pad byte");
        assert_eq!(audio.samples, samples);
    }

    #[test]
    fn a_truncated_file_is_refused() {
        assert!(matches!(decode(&[0u8; 4]), Err(WavError::Truncated { .. })));
    }

    #[test]
    fn a_non_wav_file_is_refused() {
        let mut bytes = b"RIFF".to_vec();
        bytes.extend_from_slice(&100u32.to_le_bytes());
        bytes.extend_from_slice(b"AVI ");
        bytes.extend_from_slice(&[0u8; 64]);
        assert_eq!(decode(&bytes), Err(WavError::NotWav));
    }

    #[test]
    fn compressed_containers_are_refused_by_name() {
        // A user who pointed the client at an MP3 deserves to be told that,
        // not handed "not a WAV file".
        let mut mp3 = b"ID3".to_vec();
        mp3.extend_from_slice(&[0u8; 64]);
        match decode(&mp3) {
            Err(WavError::UnsupportedContainer { format }) => assert_eq!(format, "MP3"),
            other => panic!("expected an MP3 rejection, got {other:?}"),
        }

        let mut ogg = b"OggS".to_vec();
        ogg.extend_from_slice(&[0u8; 64]);
        assert!(matches!(
            decode(&ogg),
            Err(WavError::UnsupportedContainer { .. })
        ));
    }

    #[test]
    fn a_file_with_no_data_chunk_is_refused() {
        let mut fmt = Vec::new();
        fmt.extend_from_slice(&FORMAT_PCM.to_le_bytes());
        fmt.extend_from_slice(&1u16.to_le_bytes());
        fmt.extend_from_slice(&16_000u32.to_le_bytes());
        fmt.extend_from_slice(&32_000u32.to_le_bytes());
        fmt.extend_from_slice(&2u16.to_le_bytes());
        fmt.extend_from_slice(&16u16.to_le_bytes());

        let mut out = Vec::new();
        out.extend_from_slice(b"RIFF");
        out.extend_from_slice(&0u32.to_le_bytes());
        out.extend_from_slice(b"WAVE");
        out.extend_from_slice(b"fmt ");
        out.extend_from_slice(&(fmt.len() as u32).to_le_bytes());
        out.extend_from_slice(&fmt);

        assert_eq!(decode(&out), Err(WavError::MissingDataChunk));
    }

    #[test]
    fn a_file_with_no_format_chunk_is_refused() {
        let mut out = Vec::new();
        out.extend_from_slice(b"RIFF");
        out.extend_from_slice(&0u32.to_le_bytes());
        out.extend_from_slice(b"WAVE");
        out.extend_from_slice(b"data");
        out.extend_from_slice(&4u32.to_le_bytes());
        out.extend_from_slice(&[0u8; 4]);

        assert_eq!(decode(&out), Err(WavError::MissingFormatChunk));
    }

    #[test]
    fn an_unsupported_encoding_names_the_code_and_depth() {
        // 12-bit PCM is legal WAV that this decoder does not read.
        let file = wav(FORMAT_PCM, 1, 16_000, 12, &[0u8; 8]);
        match decode(&file) {
            Err(WavError::UnsupportedEncoding { code, bits }) => {
                assert_eq!(code, FORMAT_PCM);
                assert_eq!(bits, 12);
            }
            other => panic!("expected an encoding rejection, got {other:?}"),
        }
    }

    #[test]
    fn a_zero_channel_header_is_refused_rather_than_dividing_by_zero() {
        let file = wav(FORMAT_PCM, 0, 16_000, 16, &[0u8; 8]);
        assert!(matches!(
            decode(&file),
            Err(WavError::MalformedFormat { .. })
        ));
    }

    #[test]
    fn a_zero_sample_rate_header_is_refused() {
        let file = wav(FORMAT_PCM, 1, 0, 16, &[0u8; 8]);
        assert!(matches!(
            decode(&file),
            Err(WavError::MalformedFormat { .. })
        ));
    }

    #[test]
    fn an_empty_data_chunk_decodes_to_no_samples() {
        // A zero-length recording is not an error.
        let file = wav(FORMAT_PCM, 1, 16_000, 16, &[]);
        let audio = decode(&file).unwrap();
        assert!(audio.samples.is_empty());
        assert_eq!(audio.frame_count(), 0);
    }

    #[test]
    fn a_trailing_partial_sample_is_ignored() {
        // A byte count that is not a whole number of samples must not panic.
        let file = wav(FORMAT_PCM, 1, 16_000, 16, &[1, 2, 3]);
        let audio = decode(&file).unwrap();
        assert_eq!(audio.samples.len(), 1);
    }

    #[test]
    fn errors_read_as_sentences() {
        for error in [
            WavError::NotWav,
            WavError::MissingDataChunk,
            WavError::MissingFormatChunk,
            WavError::Truncated {
                what: "the header".into(),
            },
            WavError::UnsupportedContainer {
                format: "MP3".into(),
            },
            WavError::UnsupportedEncoding {
                code: 1,
                bits: 12,
            },
            WavError::MalformedFormat {
                detail: "zero channels".into(),
            },
        ] {
            let message = error.to_string();
            assert!(!message.is_empty(), "{error:?}");
            assert!(!message.contains("Err"), "{error:?}: {message}");
        }
    }

    #[test]
    fn sample_format_widths_are_right() {
        assert_eq!(SampleFormat::U8.bytes(), 1);
        assert_eq!(SampleFormat::I16.bytes(), 2);
        assert_eq!(SampleFormat::I24.bytes(), 3);
        assert_eq!(SampleFormat::I32.bytes(), 4);
        assert_eq!(SampleFormat::F32.bytes(), 4);
    }
}