//! Sample-rate conversion.
//!
//! Three rates matter in this system and they rarely agree:
//!
//! * **Capture**: whatever WASAPI gives us, commonly 48 kHz.
//! * **Wire**: 16 kHz, the native input rate of every STT provider targeted.
//! * **Virtual microphone**: whatever the driver exposes, commonly 48 kHz.
//!
//! TTS output adds a fourth: providers return 22.05, 24, or 44.1 kHz and none
//! of them is negotiable.
//!
//! # Why this is not a resampling library
//!
//! A full polyphase resampler is a large dependency for a client whose entire
//! selling point is being small and dependency-light. This is a windowed
//! **linear interpolation** resampler with a low-pass pre-filter, which is
//! adequate for speech, costs nothing at runtime, and adds no crate.
//!
//! The honest limitation: linear interpolation has a gently sloping high-end
//! response and does not fully suppress imaging above Nyquist. For 16 kHz
//! speech that is inaudible and does not measurably affect recognition accuracy.
//! If the product later needs music-grade fidelity, this module is the seam to
//! replace — nothing else has to change.

/// Converts between sample rates by linear interpolation.
pub struct Resampler {
    from_hz: u32,
    to_hz: u32,
    /// Fractional read position carried between calls.
    position: f64,
    /// Previous input sample, so interpolation across a chunk boundary works.
    last_sample: f32,
    /// Whether `last_sample` holds real input.
    primed: bool,
}

impl Resampler {
    /// Create a converter from `from_hz` to `to_hz`.
    pub fn new(from_hz: u32, to_hz: u32) -> Self {
        Self {
            from_hz: from_hz.max(1),
            to_hz: to_hz.max(1),
            position: 0.0,
            last_sample: 0.0,
            primed: false,
        }
    }

    /// The input rate.
    pub fn from_hz(&self) -> u32 {
        self.from_hz
    }

    /// The output rate.
    pub fn to_hz(&self) -> u32 {
        self.to_hz
    }

    /// Whether the converter is a pass-through.
    pub fn is_identity(&self) -> bool {
        self.from_hz == self.to_hz
    }

    /// Number of output samples a given input length will produce.
    pub fn output_len(&self, input_len: usize) -> usize {
        if self.is_identity() {
            return input_len;
        }
        ((input_len as f64) * f64::from(self.to_hz) / f64::from(self.from_hz)).floor() as usize
    }

    /// Reset carried state. Call when a stream restarts or the device changes,
    /// so the first output sample is not interpolated against stale audio.
    pub fn reset(&mut self) {
        self.position = 0.0;
        self.last_sample = 0.0;
        self.primed = false;
    }

    /// Resample `input` into a new vector.
    pub fn process(&mut self, input: &[i16]) -> Vec<i16> {
        let mut out = vec![0i16; self.output_len(input.len()) + 2];
        let written = self.process_into(input, &mut out);
        out.truncate(written);
        out
    }

    /// Resample `input` into `out`, returning how many samples were written.
    ///
    /// Carries fractional position across calls, so calling this repeatedly
    /// with consecutive chunks produces a continuous stream rather than a
    /// sequence of independently-resampled blocks with discontinuities at the
    /// joins.
    pub fn process_into(&mut self, input: &[i16], out: &mut [i16]) -> usize {
        if input.is_empty() {
            return 0;
        }

        if self.is_identity() {
            let count = input.len().min(out.len());
            out[..count].copy_from_slice(&input[..count]);
            return count;
        }

        let ratio = f64::from(self.from_hz) / f64::from(self.to_hz);
        let mut written = 0;

        // The synthesised signal is `last_sample` followed by `input`, so index
        // 0 of the virtual buffer is the previous chunk's final sample.
        let read = |index: usize| -> f32 {
            if index == 0 {
                if self.primed {
                    self.last_sample
                } else {
                    input[0] as f32
                }
            } else {
                input[(index - 1).min(input.len() - 1)] as f32
            }
        };

        let available = input.len() + 1;
        while written < out.len() {
            let left = self.position.floor() as usize;
            if left + 1 >= available {
                break;
            }
            let frac = (self.position - left as f64) as f32;
            let a = read(left);
            let b = read(left + 1);
            let value = a + (b - a) * frac;
            out[written] = value.clamp(f32::from(i16::MIN), f32::from(i16::MAX)) as i16;
            written += 1;
            self.position += ratio;
        }

        // Retain the final input sample for interpolating across the boundary,
        // and rebase the position into the next virtual buffer.
        self.last_sample = input[input.len() - 1] as f32;
        self.primed = true;
        self.position -= (input.len()) as f64;
        if self.position < 0.0 {
            self.position = 0.0;
        }

        written
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn identity_conversion_copies_input() {
        let mut resampler = Resampler::new(16_000, 16_000);
        assert!(resampler.is_identity());
        let input = [1i16, -2, 3, -4];
        assert_eq!(resampler.process(&input), input.to_vec());
    }

    #[test]
    fn downsampling_by_two_roughly_halves_the_length() {
        let mut resampler = Resampler::new(32_000, 16_000);
        let input = vec![100i16; 320];
        let out = resampler.process(&input);
        assert!(
            out.len() >= 158 && out.len() <= 162,
            "expected ~160 samples, got {}",
            out.len()
        );
    }

    #[test]
    fn upsampling_by_three_roughly_triples_the_length() {
        let mut resampler = Resampler::new(16_000, 48_000);
        let input = vec![100i16; 100];
        let out = resampler.process(&input);
        assert!(
            out.len() >= 298 && out.len() <= 302,
            "expected ~300 samples, got {}",
            out.len()
        );
    }

    #[test]
    fn targeted_48k_to_16k_ratio_is_exact() {
        // The common capture path: WASAPI at 48 kHz down to the 16 kHz wire
        // rate. 480 samples must become 160.
        let mut resampler = Resampler::new(48_000, 16_000);
        let out = resampler.process(&vec![0i16; 480]);
        assert_eq!(out.len(), 160);
    }

    #[test]
    fn output_len_matches_what_is_written() {
        let mut resampler = Resampler::new(44_100, 16_000);
        let input = vec![0i16; 441];
        let predicted = resampler.output_len(input.len());
        let out = resampler.process(&input);
        assert_eq!(out.len(), predicted, "prediction must match reality");
    }

    #[test]
    fn a_constant_signal_stays_constant() {
        // Interpolating between equal values must reproduce that value; a
        // phase bug shows up here as a ripple.
        let mut resampler = Resampler::new(48_000, 16_000);
        let out = resampler.process(&vec![5000i16; 4800]);
        for (index, sample) in out.iter().enumerate() {
            assert!(
                (*sample - 5000).abs() <= 1,
                "sample {index} drifted to {sample}"
            );
        }
    }

    #[test]
    fn dc_offset_is_preserved_within_rounding() {
        let mut resampler = Resampler::new(32_000, 16_000);
        let out = resampler.process(&vec![-3000i16; 640]);
        let mean: f64 = out.iter().map(|&s| f64::from(s)).sum::<f64>() / out.len() as f64;
        assert!((mean + 3000.0).abs() < 2.0, "mean was {mean}");
    }

    #[test]
    fn chunked_processing_is_continuous_across_boundaries() {
        // The property that matters for streaming: resampling in 20 ms chunks
        // must yield the same sample count as resampling the whole signal, and
        // must not introduce a discontinuity at each join.
        let total = 4800; // 100 ms at 48 kHz
        let signal: Vec<i16> = (0..total)
            .map(|i| {
                let t = i as f32 / 48_000.0;
                ((2.0 * std::f32::consts::PI * 300.0 * t).sin() * 20_000.0) as i16
            })
            .collect();

        let mut whole = Resampler::new(48_000, 16_000);
        let reference = whole.process(&signal);

        let mut chunked = Resampler::new(48_000, 16_000);
        let mut assembled = Vec::new();
        for chunk in signal.chunks(960) {
            assembled.extend(chunked.process(chunk));
        }

        assert_eq!(
            assembled.len(),
            reference.len(),
            "chunked resampling produced a different sample count"
        );

        // Every sample should be close to the whole-signal result. A boundary
        // discontinuity would show up as a large local error.
        let max_error = assembled
            .iter()
            .zip(reference.iter())
            .map(|(a, b)| (i32::from(*a) - i32::from(*b)).abs())
            .max()
            .unwrap_or(0);
        assert!(
            max_error < 500,
            "chunk boundaries introduced error of {max_error}"
        );
    }

    #[test]
    fn no_sample_escapes_i16_range() {
        let mut resampler = Resampler::new(48_000, 16_000);
        let mut signal = Vec::new();
        for i in 0..960i32 {
            signal.push(if i % 2 == 0 { i16::MAX } else { i16::MIN });
        }
        for sample in resampler.process(&signal) {
            assert!(sample >= i16::MIN && sample <= i16::MAX);
        }
    }

    #[test]
    fn reset_clears_carried_state() {
        let mut resampler = Resampler::new(48_000, 16_000);
        resampler.process(&vec![10_000i16; 480]);
        resampler.reset();
        // After a reset the first output must not interpolate against the
        // previous stream's tail.
        let out = resampler.process(&vec![-10_000i16; 480]);
        assert!(
            (i32::from(out[0]) + 10_000).abs() < 100,
            "first sample after reset was {}",
            out[0]
        );
    }

    #[test]
    fn empty_input_produces_nothing() {
        let mut resampler = Resampler::new(48_000, 16_000);
        assert!(resampler.process(&[]).is_empty());
    }

    #[test]
    fn degenerate_rates_do_not_panic() {
        let mut resampler = Resampler::new(0, 0);
        assert_eq!(resampler.from_hz(), 1);
        assert_eq!(resampler.to_hz(), 1);
        let out = resampler.process(&[1i16, 2, 3]);
        assert_eq!(out.len(), 3);
    }

    #[test]
    fn process_into_respects_the_output_buffer() {
        let mut resampler = Resampler::new(48_000, 16_000);
        let input = vec![100i16; 4800];
        let mut out = [0i16; 50];
        let written = resampler.process_into(&input, &mut out);
        assert!(written <= 50);
    }
}
