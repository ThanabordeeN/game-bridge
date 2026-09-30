//! Put real recorded speech through the local audio path and report what the
//! VAD does with it.
//!
//! ```text
//! WAV file ──► decode ──► downmix ──► resample to 16 kHz ──► VAD ──► segments
//!                                                                      │
//!                                            optionally ──► Deepgram ◄─┘
//! ```
//!
//! # Why this exists
//!
//! Every VAD test before this one ran against synthetic signals — sine waves
//! and generated noise. Those tests prove the *logic* is right, but they cannot
//! tell whether the defaults are right, because a sine wave is not a person
//! talking. Onset frames, the hangover, and the noise margin were all chosen
//! from reasoning rather than from data.
//!
//! This runs the real path over a real recording and reports what came out:
//! how many segments, how long each is, how much of the file the VAD called
//! speech, and — with `--stt` — what the transcription of each segment actually
//! says.
//!
//! # The number that matters
//!
//! **Speech fraction.** If the VAD calls 90% of a recording "speech", it is not
//! filtering anything and the billing promise in SPEC §29 is not being kept. If
//! it calls 10% of continuous speech "speech", it is clipping words. Both
//! failures look identical from a passing unit test.
//!
//! ```bash
//! cargo run -p game-bridge-client --example segments -- clip.wav
//! DEEPGRAM_API_KEY=... cargo run -p game-bridge-client --example segments -- --stt clip.wav
//! ```

use std::time::Instant;

use game_bridge_client::audio::resampler::Resampler;
use game_bridge_client::audio::vad::{Vad, VadConfig, VadVerdict};
use game_bridge_client::audio::wav;
use game_bridge_client::transcribe::{
    deepgram::{Deepgram, DeepgramConfig},
    is_translatable, AudioInput, TranscriptionProvider, DEFAULT_DEEPGRAM_KEY_ENV,
};

/// What the wire expects.
const WIRE_RATE: u32 = 16_000;

fn main() {
    let args: Vec<String> = std::env::args().skip(1).collect();
    let run_stt = args.iter().any(|a| a == "--stt");
    let verbose = args.iter().any(|a| a == "--verbose");
    let Some(path) = args.iter().find(|a| !a.starts_with("--")).cloned() else {
        eprintln!("usage: segments [--stt] [--verbose] <audio.wav>");
        std::process::exit(2);
    };

    let bytes = match std::fs::read(&path) {
        Ok(bytes) => bytes,
        Err(error) => {
            eprintln!("Cannot read {path}: {error}");
            std::process::exit(2);
        }
    };

    // ---- decode ----
    let audio = match wav::decode(&bytes) {
        Ok(audio) => audio,
        Err(error) => {
            eprintln!("Cannot decode {path}: {error}");
            std::process::exit(2);
        }
    };

    println!("File      : {path}");
    println!(
        "Format    : {} Hz, {} channel(s), {}",
        audio.sample_rate_hz,
        audio.channels,
        audio.format.label()
    );
    println!("Duration  : {:.2} s", audio.duration().as_secs_f32());

    // ---- downmix and resample to the wire rate ----
    let mono = audio.to_mono();
    let mono_seconds = mono.len() as f64 / f64::from(audio.sample_rate_hz);

    let pcm = if audio.sample_rate_hz == WIRE_RATE {
        mono
    } else {
        let mut resampler = Resampler::new(audio.sample_rate_hz, WIRE_RATE);
        let resampled = resampler.process(&mono);
        println!(
            "Resampled : {} Hz → {WIRE_RATE} Hz ({} → {} samples)",
            audio.sample_rate_hz,
            mono.len(),
            resampled.len()
        );
        resampled
    };

    // ---- energy profile ----
    // Printed before the VAD runs, because it is what explains the VAD's
    // behaviour. A detector that calls everything speech is usually a detector
    // whose noise floor never found any quiet to learn from, and the energy
    // percentiles show whether any quiet existed.
    {
        use game_bridge_client::audio::vad::{frame_energy_db, zero_crossing_rate};
        let frame_len = config_frame_len();
        let mut energies: Vec<f32> = Vec::new();
        let mut zcrs: Vec<f32> = Vec::new();
        for frame in pcm.chunks_exact(frame_len) {
            energies.push(frame_energy_db(frame));
            zcrs.push(zero_crossing_rate(frame));
        }
        if !energies.is_empty() {
            let mut sorted: Vec<f32> = energies.iter().copied().filter(|e| e.is_finite()).collect();
            sorted.sort_by(|a, b| a.partial_cmp(b).unwrap_or(std::cmp::Ordering::Equal));
            let pct = |p: f64| -> f32 {
                if sorted.is_empty() {
                    return f32::NEG_INFINITY;
                }
                sorted[((sorted.len() - 1) as f64 * p) as usize]
            };
            let above_threshold = energies
                .iter()
                .filter(|e| **e >= VadConfig::default().speech_threshold_db)
                .count();

            println!("\nEnergy profile ({} frames):", energies.len());
            println!(
                "  min {:.1}  p5 {:.1}  p25 {:.1}  p50 {:.1}  p75 {:.1}  p95 {:.1} dBFS",
                sorted.first().copied().unwrap_or(f32::NEG_INFINITY),
                pct(0.05),
                pct(0.25),
                pct(0.50),
                pct(0.75),
                pct(0.95),
            );
            println!(
                "  frames at or above the {:.0} dBFS absolute threshold: {} of {} ({:.0}%)",
                VadConfig::default().speech_threshold_db,
                above_threshold,
                energies.len(),
                above_threshold as f64 / energies.len() as f64 * 100.0
            );

            let mut sorted_zcr = zcrs.clone();
            sorted_zcr.sort_by(|a, b| a.partial_cmp(b).unwrap_or(std::cmp::Ordering::Equal));
            println!(
                "  zcr    p5 {:.3}  p50 {:.3}  p95 {:.3}  (reject above {:.2})",
                sorted_zcr[sorted_zcr.len() / 20],
                sorted_zcr[sorted_zcr.len() / 2],
                sorted_zcr[sorted_zcr.len() * 19 / 20],
                VadConfig::default().max_zcr
            );
        }
    }

    // ---- VAD ----
    // Tunable from the environment so a sweep does not need a recompile.
    let mut config = VadConfig::default();
    if let Ok(v) = std::env::var("VAD_FLOOR_WINDOW_MS") {
        if let Ok(ms) = v.parse() {
            config.floor_window_ms = ms;
        }
    }
    if let Ok(v) = std::env::var("VAD_FLOOR_TRUST_MS") {
        if let Ok(ms) = v.parse() {
            config.floor_trust_ms = ms;
        }
    }
    if let Ok(v) = std::env::var("VAD_HANGOVER_MS") {
        if let Ok(ms) = v.parse() {
            config.hangover_ms = ms;
        }
    }
    if let Ok(v) = std::env::var("VAD_MARGIN_DB") {
        if let Ok(db) = v.parse() {
            config.noise_margin_db = db;
        }
    }
    println!("\nVAD config:");
    println!(
        "  frame {} samples ({:.1} ms), onset {} frames, hangover {} ms",
        config.frame_samples,
        config.frame_ms(WIRE_RATE),
        config.onset_frames,
        config.hangover_ms
    );
    println!(
        "  pre-roll {} ms, post-roll {} ms, threshold {:.0} dBFS + {:.0} dB over floor",
        config.pre_roll_ms, config.post_roll_ms, config.speech_threshold_db, config.noise_margin_db
    );

    let mut vad = Vad::new(config, WIRE_RATE);
    let mut segments: Vec<(u64, Vec<i16>)> = Vec::new();
    let mut next_id = 0u64;
    let mut speech_samples = 0usize;
    let mut dropped_silence_samples = 0usize;
    let mut current: Option<(u64, Vec<i16>)> = None;
    // Position of each segment's first sample, so a segment covering a region
    // that should be silent is visible rather than inferred.
    let mut offsets: Vec<usize> = Vec::new();
    let mut position = 0usize;
    let mut current_start = 0usize;

    for (verdict, frame) in vad.push(&pcm) {
        match verdict {
            VadVerdict::SpeechStarted => {
                // The payload begins with the pre-roll.
                speech_samples += frame.len();
                // The pre-roll means the segment starts before the trigger.
                let pre_roll = frame.len().saturating_sub(config.frame_samples);
                current_start = position.saturating_sub(pre_roll);
                current = Some((next_id, frame));
            }
            VadVerdict::Speech => {
                speech_samples += frame.len();
                if let Some((_, buffer)) = current.as_mut() {
                    buffer.extend_from_slice(&frame);
                }
            }
            VadVerdict::SpeechEnded => {
                speech_samples += frame.len();
                if let Some((id, mut buffer)) = current.take() {
                    buffer.extend_from_slice(&frame);
                    offsets.push(current_start);
                    segments.push((id, buffer));
                    next_id += 1;
                } else {
                    offsets.push(position);
                    segments.push((next_id, frame));
                    next_id += 1;
                }
            }
            VadVerdict::Silence => {
                // A Silence verdict carries an empty payload — there is nothing
                // to emit — so the frame length has to come from the config.
                // Counting `frame.len()` here silently reports zero dropped
                // silence, which reads as "the VAD filters nothing".
                dropped_silence_samples += config.frame_samples;
            }
        }
        position += config.frame_samples;
    }

    // Anything still open at the end of the file.
    if let Some((id, mut buffer)) = current.take() {
        if let Some(tail) = vad.flush() {
            buffer.extend_from_slice(&tail);
        }
        offsets.push(current_start);
        segments.push((id, buffer));
    }

    // ---- report ----
    let total_samples = pcm.len().max(1);
    // Measured from the segments themselves. Summing emitted payloads would
    // double-count the pre-roll, which replays frames that were also seen as
    // Silence before the onset was confirmed.
    let segment_samples: usize = segments.iter().map(|(_, s)| s.len()).sum();
    let speech_fraction = segment_samples as f64 / total_samples as f64;

    println!("\n--- segmentation ---");
    println!("  audio          : {mono_seconds:.2} s");
    println!("  segments       : {}", segments.len());
    println!(
        "  speech         : {:.2} s ({:.1}% of the file)",
        segment_samples as f64 / f64::from(WIRE_RATE),
        speech_fraction * 100.0
    );
    let _ = speech_samples;
    println!(
        "  dropped silence: {:.2} s",
        dropped_silence_samples as f64 / f64::from(WIRE_RATE)
    );

    if !segments.is_empty() {
        let mut durations: Vec<f64> = segments
            .iter()
            .map(|(_, s)| s.len() as f64 / f64::from(WIRE_RATE))
            .collect();
        durations.sort_by(|a, b| a.partial_cmp(b).unwrap_or(std::cmp::Ordering::Equal));
        let median = durations[durations.len() / 2];
        let total: f64 = durations.iter().sum();
        println!(
            "  segment length : min {:.2} s, median {:.2} s, max {:.2} s",
            durations.first().copied().unwrap_or(0.0),
            median,
            durations.last().copied().unwrap_or(0.0)
        );
        println!(
            "  mean length    : {:.2} s",
            total / durations.len() as f64
        );
    }

    // A rough verdict, so the numbers are actionable rather than decorative.
    println!("\n--- assessment ---");
    if segments.is_empty() {
        println!("  No speech detected. If the file contains speech, the threshold is too high.");
    } else if speech_fraction > 0.85 {
        println!(
            "  Speech fraction {:.0}% is very high. The VAD is barely filtering, so silence \
             would be sent to the cloud and billed.",
            speech_fraction * 100.0
        );
    } else if speech_fraction < 0.10 && mono_seconds > 5.0 {
        println!(
            "  Speech fraction {:.0}% is very low. Words are likely being clipped.",
            speech_fraction * 100.0
        );
    } else {
        println!(
            "  Speech fraction {:.0}% is in a plausible range for conversational audio.",
            speech_fraction * 100.0
        );
    }

    let short = segments
        .iter()
        .filter(|(_, s)| s.len() < (WIRE_RATE as usize / 4))
        .count();
    if short > 0 {
        println!(
            "  {short} segment(s) are under 250 ms. Very short segments are usually a click or a \
             cough rather than a word."
        );
    }

    if verbose {
        println!("\n--- segments ---");
        for ((id, samples), offset) in segments.iter().zip(offsets.iter()) {
            println!(
                "  {:02}  {:>6.2}s → {:>6.2}s   {:>5.2} s long",
                id,
                *offset as f64 / f64::from(WIRE_RATE),
                (*offset + samples.len()) as f64 / f64::from(WIRE_RATE),
                samples.len() as f64 / f64::from(WIRE_RATE),
            );
        }
    }

    if !run_stt {
        println!("\n(pass --stt to transcribe each segment through Deepgram)");
        return;
    }

    // ---- speech to text, per segment ----
    let transcriber = match Deepgram::from_env(DeepgramConfig::default()) {
        Ok(transcriber) => transcriber,
        Err(error) => {
            eprintln!(
                "\nSpeech-to-text unavailable: {}\nSet {DEFAULT_DEEPGRAM_KEY_ENV} to use --stt.",
                error.user_message()
            );
            std::process::exit(2);
        }
    };

    println!("\n--- transcription ---");
    println!("  provider: {}", transcriber.describe());

    let mut stt_times = Vec::new();
    let mut transcribed_words = 0usize;
    let mut skipped = 0usize;

    for (id, samples) in &segments {
        let wav_bytes = encode_wav_i16(samples, WIRE_RATE);
        let input = AudioInput::Bytes {
            data: &wav_bytes,
            content_type: "audio/wav",
        };

        let started = Instant::now();
        match transcriber.transcribe(&input) {
            Ok(result) => {
                let elapsed = started.elapsed();
                stt_times.push(elapsed.as_secs_f64() * 1000.0);
                let words = result.text.split_whitespace().count();
                transcribed_words += words;

                let language = result
                    .detected_language
                    .map(|l| l.display_name().to_string())
                    .unwrap_or_else(|| "?".into());

                if is_translatable(&result) {
                    println!(
                        "  {:02}  {:>5.0} ms  [{}] {}",
                        id,
                        elapsed.as_secs_f64() * 1000.0,
                        language,
                        result.text
                    );
                } else {
                    skipped += 1;
                    println!(
                        "  {:02}  {:>5.0} ms  skipped (nothing translatable, confidence {:.2})",
                        id,
                        elapsed.as_secs_f64() * 1000.0,
                        result.confidence
                    );
                }
            }
            Err(error) => {
                println!("  {:02}  failed: {}", id, error.user_message());
            }
        }
    }

    if !stt_times.is_empty() {
        let mut sorted = stt_times.clone();
        sorted.sort_by(|a, b| a.partial_cmp(b).unwrap_or(std::cmp::Ordering::Equal));
        println!(
            "\n  stt latency: median {:.0} ms, min {:.0}, max {:.0}",
            sorted[sorted.len() / 2],
            sorted.first().copied().unwrap_or(0.0),
            sorted.last().copied().unwrap_or(0.0)
        );
        println!(
            "  words transcribed: {transcribed_words} across {} segments ({skipped} skipped)",
            stt_times.len()
        );
    }
}

/// The VAD's analysis frame length in samples.
fn config_frame_len() -> usize {
    VadConfig::default().frame_samples
}

/// Encode mono 16-bit PCM as a WAV file, so it can be posted.
///
/// The capture path sends raw PCM, but a self-describing container is easier to
/// verify by hand and the provider accepts either.
fn encode_wav_i16(samples: &[i16], sample_rate_hz: u32) -> Vec<u8> {
    let data_len = (samples.len() * 2) as u32;
    let mut out = Vec::with_capacity(44 + samples.len() * 2);

    out.extend_from_slice(b"RIFF");
    out.extend_from_slice(&(36 + data_len).to_le_bytes());
    out.extend_from_slice(b"WAVE");
    out.extend_from_slice(b"fmt ");
    out.extend_from_slice(&16u32.to_le_bytes());
    out.extend_from_slice(&1u16.to_le_bytes()); // PCM
    out.extend_from_slice(&1u16.to_le_bytes()); // mono
    out.extend_from_slice(&sample_rate_hz.to_le_bytes());
    out.extend_from_slice(&(sample_rate_hz * 2).to_le_bytes());
    out.extend_from_slice(&2u16.to_le_bytes()); // block align
    out.extend_from_slice(&16u16.to_le_bytes()); // bits
    out.extend_from_slice(b"data");
    out.extend_from_slice(&data_len.to_le_bytes());
    for sample in samples {
        out.extend_from_slice(&sample.to_le_bytes());
    }
    out
}

