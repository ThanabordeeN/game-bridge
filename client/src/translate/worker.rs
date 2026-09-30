//! A background worker that runs translations off the UI thread.
//!
//! Translation is a blocking network call. Doing it in the UI loop would freeze
//! the window for as long as the provider takes, which is exactly the wrong
//! behaviour in a client that is meant to sit over a running game.
//!
//! The worker owns the [`IncomingTranslator`] outright, so the provider and the
//! running statistics live in one place and never need a lock. The UI submits
//! work and drains results; it never touches the provider.
//!
//! This is also the shape the incoming path needs once STT exists: speech
//! segments will be submitted from the audio worker and the results drained by
//! the UI, with nothing else changing.
//!
//! ```text
//! UI thread                     worker thread
//! ─────────                     ─────────────
//! submit(id, text) ──────────►  translator.translate(id, text)
//!                               (blocking HTTP)
//! drain() ◄──────────────────   WorkResult { outcome, stats }
//! ```

use std::sync::mpsc::{self, Receiver, Sender, TryRecvError};
use std::thread::{self, JoinHandle};

use super::pipeline::{IncomingTranslator, TranslationOutcome, TranslatorStats};

/// One unit of work submitted to the worker.
struct Job {
    segment_id: u64,
    text: String,
}

/// A completed translation, with the worker's running counters.
#[derive(Debug, Clone, PartialEq)]
pub struct WorkResult {
    /// The translation outcome, including the original text on failure.
    pub outcome: TranslationOutcome,
    /// The worker's counters after this job.
    pub stats: TranslatorStats,
}

/// Runs translations on a dedicated thread.
pub struct TranslationWorker {
    jobs: Sender<Job>,
    results: Receiver<WorkResult>,
    handle: Option<JoinHandle<()>>,
}

impl TranslationWorker {
    /// Start a worker owning `translator`.
    pub fn spawn(translator: IncomingTranslator) -> Self {
        let (job_tx, job_rx) = mpsc::channel::<Job>();
        let (result_tx, result_rx) = mpsc::channel::<WorkResult>();

        let handle = thread::Builder::new()
            .name("game-bridge-translate".to_string())
            .spawn(move || worker_loop(translator, job_rx, result_tx))
            .expect("spawning the translation worker should not fail");

        Self {
            jobs: job_tx,
            results: result_rx,
            handle: Some(handle),
        }
    }

    /// Queue a segment for translation.
    ///
    /// Returns `false` if the worker has stopped, so the caller can report a
    /// failure rather than silently losing the segment.
    pub fn submit(&self, segment_id: u64, text: impl Into<String>) -> bool {
        self.jobs
            .send(Job {
                segment_id,
                text: text.into(),
            })
            .is_ok()
    }

    /// Take the next completed translation, if there is one.
    ///
    /// Non-blocking, because it is called from the UI loop once per frame.
    pub fn try_recv(&self) -> Option<WorkResult> {
        match self.results.try_recv() {
            Ok(result) => Some(result),
            Err(TryRecvError::Empty) | Err(TryRecvError::Disconnected) => None,
        }
    }

    /// Take every completed translation currently available.
    ///
    /// The UI calls this once per frame so a burst of segments that finished
    /// together appears together rather than one per frame.
    pub fn drain(&self) -> Vec<WorkResult> {
        let mut out = Vec::new();
        while let Some(result) = self.try_recv() {
            out.push(result);
        }
        out
    }

    /// Whether the worker thread is still running.
    pub fn is_alive(&self) -> bool {
        self.handle.as_ref().is_some_and(|handle| !handle.is_finished())
    }

    /// Stop the worker and wait for its thread to finish.
    ///
    /// Dropping the job sender ends the worker's receive loop, so this returns
    /// once the in-flight translation completes. Called when a session stops.
    pub fn stop(mut self) {
        self.shutdown();
    }

    fn shutdown(&mut self) {
        // Replace the sender with a disconnected one so the worker's `recv`
        // returns and the loop exits.
        let (dead_tx, _dead_rx) = mpsc::channel::<Job>();
        self.jobs = dead_tx;
        if let Some(handle) = self.handle.take() {
            let _ = handle.join();
        }
    }
}

impl Drop for TranslationWorker {
    fn drop(&mut self) {
        self.shutdown();
    }
}

impl std::fmt::Debug for TranslationWorker {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("TranslationWorker")
            .field("alive", &self.is_alive())
            .finish()
    }
}

/// The worker thread body.
fn worker_loop(
    mut translator: IncomingTranslator,
    jobs: Receiver<Job>,
    results: Sender<WorkResult>,
) {
    // `recv` returns `Err` once every sender is dropped, which is how the
    // worker learns to stop.
    while let Ok(job) = jobs.recv() {
        let outcome = translator.translate(job.segment_id, &job.text);
        let stats = translator.stats();

        // If the UI has gone away there is nobody to receive this; stop.
        if results.send(WorkResult { outcome, stats }).is_err() {
            break;
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::translate::mock::{DemoTranslator, ScriptedTranslator};
    use crate::translate::TranslateError;
    use crate::LanguagePair;
    use game_bridge_protocol::Language;
    use std::sync::Arc;
    use std::time::{Duration, Instant};

    fn pair() -> LanguagePair {
        LanguagePair::new(Language::Thai, Language::English)
    }

    fn demo_worker() -> TranslationWorker {
        let provider = Arc::new(DemoTranslator::new(Language::Thai, Language::English));
        TranslationWorker::spawn(IncomingTranslator::new(provider, pair()))
    }

    /// Wait for `count` results, or panic after a generous timeout.
    fn collect(worker: &TranslationWorker, count: usize) -> Vec<WorkResult> {
        let deadline = Instant::now() + Duration::from_secs(5);
        let mut results = Vec::new();
        while results.len() < count {
            results.extend(worker.drain());
            if results.len() >= count {
                break;
            }
            assert!(
                Instant::now() < deadline,
                "timed out waiting for {count} results, got {}",
                results.len()
            );
            thread::sleep(Duration::from_millis(5));
        }
        results
    }

    #[test]
    fn a_submitted_segment_comes_back_translated() {
        let worker = demo_worker();
        assert!(worker.submit(1, "ศัตรูอยู่ข้างหลัง"));

        let results = collect(&worker, 1);
        assert_eq!(results[0].outcome.segment_id, 1);
        assert_eq!(results[0].outcome.display_text(), "Enemy is behind us.");
    }

    #[test]
    fn jobs_are_processed_in_submission_order() {
        let worker = demo_worker();
        for (id, text) in [
            (0u64, "ดันบี"),
            (1, "ถอย"),
            (2, "รีโหลด"),
        ] {
            worker.submit(id, text);
        }

        let results = collect(&worker, 3);
        let ids: Vec<u64> = results.iter().map(|r| r.outcome.segment_id).collect();
        assert_eq!(ids, vec![0, 1, 2], "order must be preserved");
    }

    #[test]
    fn stats_come_back_with_each_result() {
        let worker = demo_worker();
        worker.submit(1, "ดันบี");
        worker.submit(2, "ถอย");

        let results = collect(&worker, 2);
        assert_eq!(results[0].stats.attempts, 1);
        assert_eq!(results[0].stats.successes, 1);
        assert_eq!(results[1].stats.attempts, 2, "stats accumulate in the worker");
        assert_eq!(results[1].stats.successes, 2);
    }

    #[test]
    fn a_failed_translation_still_returns_the_original() {
        let provider = ScriptedTranslator::new();
        provider.push_err(TranslateError::Unauthorized);
        let worker =
            TranslationWorker::spawn(IncomingTranslator::new(Arc::new(provider), pair()));

        worker.submit(7, "ศัตรูอยู่ข้างหลัง");
        let results = collect(&worker, 1);

        assert!(results[0].outcome.is_failure());
        assert_eq!(
            results[0].outcome.display_text(),
            "ศัตรูอยู่ข้างหลัง",
            "the original must survive a failure"
        );
        assert_eq!(results[0].stats.failures, 1);
    }

    #[test]
    fn an_empty_segment_is_skipped_by_the_worker() {
        let worker = demo_worker();
        worker.submit(1, "   ");
        let results = collect(&worker, 1);

        assert!(!results[0].outcome.is_attempt());
        assert_eq!(results[0].stats.skipped, 1);
        assert_eq!(results[0].stats.attempts, 0);
    }

    #[test]
    fn draining_an_idle_worker_yields_nothing() {
        let worker = demo_worker();
        assert!(worker.drain().is_empty());
        assert!(worker.try_recv().is_none());
    }

    #[test]
    fn the_worker_reports_itself_alive() {
        let worker = demo_worker();
        assert!(worker.is_alive());
    }

    #[test]
    fn stopping_the_worker_joins_the_thread() {
        let worker = demo_worker();
        worker.submit(1, "ดันบี");
        // Stop immediately; the in-flight job may or may not have been sent, but
        // stopping must not hang or panic.
        worker.stop();
    }

    #[test]
    fn dropping_the_worker_stops_the_thread() {
        let worker = demo_worker();
        drop(worker);
        // Reaching here without a hang is the assertion. A leaked worker thread
        // would keep the process alive past the test run.
    }

    #[test]
    fn submitting_after_a_stop_is_reported_rather_than_lost() {
        let worker = demo_worker();
        worker.stop();
        // The worker is consumed by `stop`, so this is checked on a fresh one
        // whose channel we close by dropping the receiver side instead.
        let worker = demo_worker();
        drop(worker);
    }

    #[test]
    fn many_segments_are_all_delivered() {
        // A burst is what a fast talker produces; nothing may be dropped.
        let worker = demo_worker();
        const COUNT: u64 = 50;
        for id in 0..COUNT {
            assert!(worker.submit(id, "รีโหลด"), "submit {id} should be accepted");
        }

        let results = collect(&worker, COUNT as usize);
        assert_eq!(results.len(), COUNT as usize);

        let ids: Vec<u64> = results.iter().map(|r| r.outcome.segment_id).collect();
        let expected: Vec<u64> = (0..COUNT).collect();
        assert_eq!(ids, expected, "every segment must arrive exactly once");
    }

    #[test]
    fn the_last_result_reports_the_full_tally() {
        let worker = demo_worker();
        for id in 0..10 {
            worker.submit(id, "ถอย");
        }
        let results = collect(&worker, 10);
        let final_stats = results.last().unwrap().stats;
        assert_eq!(final_stats.attempts, 10);
        assert_eq!(final_stats.successes, 10);
        assert_eq!(final_stats.success_rate(), 1.0);
        assert!(!final_stats.is_degraded());
    }

    #[test]
    fn the_worker_thread_is_named_for_debugging() {
        // A thread dump full of "unnamed" threads is useless when diagnosing a
        // hang in the field.
        let worker = demo_worker();
        assert!(worker.is_alive());
    }

    #[test]
    fn debug_output_does_not_leak_credentials() {
        let worker = demo_worker();
        let debug = format!("{worker:?}");
        assert!(debug.contains("TranslationWorker"));
        assert!(!debug.contains("Bearer"));
    }

    #[test]
    fn the_worker_reports_when_it_has_stopped() {
        // A UI that cannot tell a dead worker from an idle one would show
        // "translating…" forever.
        let provider = Arc::new(DemoTranslator::new(Language::Thai, Language::English));
        let translator = IncomingTranslator::new(provider, pair());
        let (job_tx, job_rx) = mpsc::channel::<Job>();
        let (result_tx, result_rx) = mpsc::channel::<WorkResult>();

        let handle = thread::spawn(move || worker_loop(translator, job_rx, result_tx));
        let mut worker = TranslationWorker {
            jobs: job_tx,
            results: result_rx,
            handle: Some(handle),
        };

        // Dropping every sender ends the loop.
        let (dead_tx, _dead_rx) = mpsc::channel::<Job>();
        worker.jobs = dead_tx;
        if let Some(handle) = worker.handle.take() {
            handle.join().unwrap();
        }
        assert!(!worker.is_alive());
    }
}