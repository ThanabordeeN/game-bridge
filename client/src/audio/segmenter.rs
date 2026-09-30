//! Turning captured audio into utterances.
//!
//! The VAD knows *when* someone is speaking; this module turns that into the
//! discrete utterances the rest of the pipeline works in — each with the audio
//! to transcribe and the duration to bill.
//!
//! # Billing honesty
//!
//! A segment is not pure speech. It carries pre-roll before the onset and
//! hangover after the offset, both of which exist to avoid clipping words and
//! both of which are silence. Billing the raw segment length would quietly
//! charge for that padding on every utterance.
//!
//! So a [`Segment`] reports two durations: `audio_ms`, which is what gets sent
//! to the provider, and `speech_ms`, which excludes the known padding and is
//! what the meter counts. The difference is a few hundred milliseconds per
//! utterance, which at ฿2.50/minute is small — but it is the user's money, and
//! "small per utterance" is not the same as "correct".

use crate::audio::vad::{Vad, VadConfig, VadVerdict};

/// The rate the rest of the pipeline works at.
pub const WIRE_RATE: u32 = 16_000;

/// One utterance.
#[derive(Debug, Clone, PartialEq)]
pub struct Segment {
    /// Index within the audio that produced it.
    pub index: u32,
    /// Mono 16 kHz PCM, including pre-roll and hangover.
    pub samples: Vec<i16>,
    /// Where the segment starts, in milliseconds from the beginning of the
    /// input. Includes the pre-roll.
    pub start_ms: u64,
    /// Length of [`Segment::samples`] in milliseconds.
    pub audio_ms: u64,
    /// Estimated voiced duration, excluding the pre-roll and hangover padding.
    ///
    /// This is the quantity the active-voice meter counts.
    pub speech_ms: u64,
}

impl Segment {
    /// Duration in samples.
    pub fn len(&self) -> usize {
        self.samples.len()
    }

    /// Whether the segment has no audio.
    pub fn is_empty(&self) -> bool {
        self.samples.is_empty()
    }
}

/// Split mono PCM into utterances.
pub fn segment(pcm: &[i16], config: VadConfig, sample_rate_hz: u32) -> Vec<Segment> {
    let mut vad = Vad::new(config, sample_rate_hz);

    let pre_roll_samples = config.frames_for_ms(config.pre_roll_ms, sample_rate_hz)
        * config.frame_samples;
    let hangover_samples = config.frames_for_ms(config.hangover_ms, sample_rate_hz)
        * config.frame_samples;

    let mut out: Vec<Segment> = Vec::new();
    let mut current: Option<(usize, Vec<i16>)> = None;
    let mut position = 0usize;

    let finish = |start: usize, buffer: Vec<i16>, out: &mut Vec<Segment>| {
        if buffer.is_empty() {
            return;
        }
        let audio_samples = buffer.len();
        // The padding is known exactly: the pre-roll is prepended at onset and
        // the hangover is the trailing silence that ended it. Subtract both,
        // but never report less than one frame — a segment that exists at all
        // contained something.
        let padding = pre_roll_samples.saturating_add(hangover_samples);
        let speech_samples = audio_samples
            .saturating_sub(padding)
            .max(config.frame_samples);

        out.push(Segment {
            index: out.len() as u32,
            samples: buffer,
            start_ms: (start as u64) * 1000 / u64::from(sample_rate_hz.max(1)),
            audio_ms: (audio_samples as u64) * 1000 / u64::from(sample_rate_hz.max(1)),
            speech_ms: (speech_samples as u64) * 1000 / u64::from(sample_rate_hz.max(1)),
        });
    };

    for (verdict, frame) in vad.push(pcm) {
        match verdict {
            VadVerdict::SpeechStarted => {
                // The payload begins with the pre-roll, so the segment starts
                // before the trigger.
                let pre_roll = frame.len().saturating_sub(config.frame_samples);
                let start = position.saturating_sub(pre_roll);
                current = Some((start, frame));
            }
            VadVerdict::Speech => {
                if let Some((_, buffer)) = current.as_mut() {
                    buffer.extend_from_slice(&frame);
                }
            }
            VadVerdict::SpeechEnded => {
                if let Some((start, mut buffer)) = current.take() {
                    buffer.extend_from_slice(&frame);
                    finish(start, buffer, &mut out);
                }
            }
            VadVerdict::Silence => {}
        }
        position += config.frame_samples;
    }

    // Anything still open when the audio runs out.
    if let Some((start, mut buffer)) = current.take() {
        if let Some(tail) = vad.flush() {
            buffer.extend_from_slice(&tail);
        }
        finish(start, buffer, &mut out);
    }

    out
}

/// Convert decoded audio into the mono 16 kHz PCM the pipeline expects.
pub fn to_wire_pcm(audio: &crate::audio::wav::WavAudio) -> Vec<i16> {
    let mono = audio.to_mono();
    if audio.sample_rate_hz == WIRE_RATE {
        return mono;
    }
    let mut resampler = crate::audio::resampler::Resampler::new(audio.sample_rate_hz, WIRE_RATE);
    resampler.process(&mono)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::audio::vad::sine_wave;

    /// A speech-like frame: a voiced carrier under a syllabic envelope.
    ///
    /// A constant tone is not usable here for the same reason it is not usable
    /// in the VAD's own tests — a steady signal has a noise floor equal to
    /// itself, so the detector correctly treats it as background.
    fn speech_frame_at(index: usize) -> Vec<i16> {
        let phase = ((index + 2) % 8) as f32 / 8.0;
        let envelope = 0.08 + 0.62 * (std::f32::consts::PI * phase).sin();
        sine_wave(200.0, envelope, WIRE_RATE, 160)
    }

    fn silence(frames: usize) -> Vec<i16> {
        vec![0i16; frames * 160]
    }

    fn speech(frames: usize) -> Vec<i16> {
        (0..frames)
            .flat_map(|index| speech_frame_at(index))
            .collect()
    }

    #[test]
    fn a_single_utterance_becomes_one_segment() {
        let mut pcm = silence(20);
        pcm.extend(speech(30));
        pcm.extend(silence(60));

        let segments = segment(&pcm, VadConfig::default(), WIRE_RATE);
        assert_eq!(segments.len(), 1);
        assert_eq!(segments[0].index, 0);
        assert!(!segments[0].is_empty());
    }

    #[test]
    fn two_utterances_become_two_segments() {
        let mut pcm = Vec::new();
        pcm.extend(speech(30));
        pcm.extend(silence(80));
        pcm.extend(speech(30));
        pcm.extend(silence(80));

        let segments = segment(&pcm, VadConfig::default(), WIRE_RATE);
        assert_eq!(segments.len(), 2);
        assert_eq!(segments[0].index, 0);
        assert_eq!(segments[1].index, 1);
    }

    #[test]
    fn pure_silence_produces_no_segments() {
        let segments = segment(&silence(200), VadConfig::default(), WIRE_RATE);
        assert!(segments.is_empty());
    }

    #[test]
    fn a_single_segment_includes_its_pre_roll() {
        // The pre-roll exists so the first phoneme is not clipped, so the
        // segment must start before the trigger.
        let config = VadConfig {
            onset_frames: 1,
            pre_roll_ms: 200,
            ..Default::default()
        };
        let mut pcm = silence(30);
        pcm.extend(speech(30));
        pcm.extend(silence(80));

        let segments = segment(&pcm, config, WIRE_RATE);
        assert_eq!(segments.len(), 1);
        // Silence began at 0.3 s; a 200 ms pre-roll pulls the start back.
        assert!(
            segments[0].start_ms < 300,
            "start was {} ms, expected the pre-roll to pull it earlier",
            segments[0].start_ms
        );
    }

    #[test]
    fn speech_time_excludes_the_padding() {
        // The property that matters for billing: a segment carries silence at
        // both ends, and the meter must not count it.
        let config = VadConfig::default();
        let mut pcm = speech(30);
        pcm.extend(silence(80));

        let segments = segment(&pcm, config, WIRE_RATE);
        assert_eq!(segments.len(), 1);
        let segment = &segments[0];

        assert!(
            segment.speech_ms < segment.audio_ms,
            "speech {} ms should be less than audio {} ms",
            segment.speech_ms,
            segment.audio_ms
        );
        // The padding is the pre-roll plus the hangover, which the defaults set
        // to 300 ms + 500 ms.
        let expected_padding = 800;
        let actual_padding = segment.audio_ms - segment.speech_ms;
        assert!(
            actual_padding >= expected_padding - 120,
            "padding was {actual_padding} ms, expected roughly {expected_padding} ms"
        );
    }

    #[test]
    fn speech_time_never_underflows_to_zero() {
        // A very short segment has more padding than audio. Reporting a
        // negative or zero duration would make the meter wrong in the other
        // direction.
        let config = VadConfig {
            onset_frames: 1,
            pre_roll_ms: 2_000,
            hangover_ms: 2_000,
            ..Default::default()
        };
        let mut pcm = speech(5);
        pcm.extend(silence(10));

        for segment in segment(&pcm, config, WIRE_RATE) {
            assert!(segment.speech_ms > 0, "a segment must bill something");
            assert!(segment.speech_ms <= segment.audio_ms);
        }
    }

    #[test]
    fn every_segment_is_indexed_in_order() {
        let mut pcm = Vec::new();
        for _ in 0..3 {
            pcm.extend(speech(25));
            pcm.extend(silence(80));
        }
        let segments = segment(&pcm, VadConfig::default(), WIRE_RATE);
        assert_eq!(segments.len(), 3);
        for (index, segment) in segments.iter().enumerate() {
            assert_eq!(segment.index, index as u32);
        }
    }

    #[test]
    fn segments_are_ordered_by_start_time() {
        let mut pcm = Vec::new();
        for _ in 0..3 {
            pcm.extend(speech(25));
            pcm.extend(silence(80));
        }
        let segments = segment(&pcm, VadConfig::default(), WIRE_RATE);
        for pair in segments.windows(2) {
            assert!(
                pair[0].start_ms < pair[1].start_ms,
                "segments out of order: {} then {}",
                pair[0].start_ms,
                pair[1].start_ms
            );
        }
    }

    #[test]
    fn segment_durations_are_plausible() {
        let mut pcm = speech(40);
        pcm.extend(silence(100));
        let segments = segment(&pcm, VadConfig::default(), WIRE_RATE);
        assert_eq!(segments.len(), 1);
        let segment = &segments[0];
        assert!(
            (300..=900).contains(&segment.audio_ms),
            "audio duration {} ms is implausible for 400 ms of speech",
            segment.audio_ms
        );
    }

    #[test]
    fn audio_that_never_ends_is_still_flushed() {
        // A recording that stops mid-sentence must not lose the last utterance.
        let config = VadConfig {
            onset_frames: 1,
            hangover_ms: 10_000,
            ..Default::default()
        };
        let pcm = speech(30);
        let segments = segment(&pcm, config, WIRE_RATE);
        assert_eq!(segments.len(), 1, "the open utterance should be flushed");
    }

    #[test]
    fn a_resampled_file_reports_time_in_the_wire_rate() {
        // Positions are computed against the rate the segmenter runs at, not
        // the file's rate, so a 48 kHz source does not report 3x the duration.
        let audio = crate::audio::wav::WavAudio {
            sample_rate_hz: 48_000,
            channels: 1,
            format: crate::audio::wav::SampleFormat::I16,
            samples: vec![0i16; 48_000],
        };
        let pcm = to_wire_pcm(&audio);
        assert_eq!(pcm.len(), 16_000, "one second at 16 kHz");

        let segments = segment(&pcm, VadConfig::default(), WIRE_RATE);
        assert!(segments.is_empty(), "silence produces nothing");
    }

    #[test]
    fn stereo_input_is_downmixed_before_segmentation() {
        let audio = crate::audio::wav::WavAudio {
            sample_rate_hz: 16_000,
            channels: 2,
            format: crate::audio::wav::SampleFormat::I16,
            // Interleaved, 100 frames.
            samples: vec![1000i16; 200],
        };
        let pcm = to_wire_pcm(&audio);
        assert_eq!(pcm.len(), 100);
    }

    #[test]
    fn empty_input_produces_nothing() {
        assert!(segment(&[], VadConfig::default(), WIRE_RATE).is_empty());
    }
}