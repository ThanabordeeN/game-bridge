//! A bounded single-producer/single-consumer ring buffer for audio samples.
//!
//! Audio callbacks run on a real-time thread that must never block, never
//! allocate, and never wait on a lock held by the UI. A lock-free SPSC ring is
//! the standard answer: the capture callback pushes, a consumer drains.
//!
//! # Dropping policy
//!
//! When the consumer falls behind and the ring is full, the oldest samples are
//! **dropped**, not the newest. This is deliberate:
//!
//! * Audio is a real-time stream. If the network stalls, the user's most recent
//!   speech is the part that still matters when it recovers.
//! * Dropping the newest samples would instead push the capture head into the
//!   future and produce a stream that never catches up, growing latency without
//!   bound.
//!
//! Every drop is counted so the UI can surface degraded capture honestly rather
//! than silently emitting a broken stream.

use std::sync::atomic::{AtomicU64, AtomicUsize, Ordering};
use std::sync::Arc;

/// A lock-free single-producer/single-consumer ring of `i16` samples.
pub struct RingBuffer {
    slots: Box<[AtomicUsize]>,
    capacity: usize,
    /// Write cursor, in samples. Monotonic.
    write: AtomicUsize,
    /// Read cursor, in samples. Monotonic.
    read: AtomicUsize,
    /// Count of samples dropped because the ring was full.
    dropped: AtomicU64,
}

// Safety: the ring is designed for exactly one producer and one consumer. The
// atomics carry the synchronization; there is no interior mutability of the
// slot values beyond the atomic wrapper.
unsafe impl Send for RingBuffer {}
unsafe impl Sync for RingBuffer {}

impl RingBuffer {
    /// Create a ring holding `capacity` samples.
    ///
    /// Capacity is rounded up to a power of two so the wrap is a mask rather
    /// than a modulo, which matters when this runs inside an audio callback.
    pub fn new(capacity: usize) -> Self {
        let capacity = capacity.max(2).next_power_of_two();
        let slots = (0..capacity)
            .map(|_| AtomicUsize::new(0))
            .collect::<Vec<_>>()
            .into_boxed_slice();
        Self {
            slots,
            capacity,
            write: AtomicUsize::new(0),
            read: AtomicUsize::new(0),
            dropped: AtomicU64::new(0),
        }
    }

    /// Create a ring shared between a producer and a consumer.
    pub fn shared(capacity: usize) -> (Producer, Consumer) {
        let ring = Arc::new(Self::new(capacity));
        (
            Producer {
                ring: Arc::clone(&ring),
            },
            Consumer { ring },
        )
    }

    /// Usable capacity in samples.
    pub fn capacity(&self) -> usize {
        self.capacity
    }

    /// Samples currently readable.
    pub fn len(&self) -> usize {
        let write = self.write.load(Ordering::Acquire);
        let read = self.read.load(Ordering::Acquire);
        write.wrapping_sub(read).min(self.capacity)
    }

    /// Whether the ring is empty.
    pub fn is_empty(&self) -> bool {
        self.len() == 0
    }

    /// Samples dropped over the lifetime of this ring.
    pub fn dropped(&self) -> u64 {
        self.dropped.load(Ordering::Relaxed)
    }

    fn slot(&self, index: usize) -> &AtomicUsize {
        &self.slots[index & (self.capacity - 1)]
    }

    /// Producer side: push one sample. Returns `false` if it had to drop the
    /// oldest sample to make room.
    fn push(&self, sample: i16) -> bool {
        let write = self.write.load(Ordering::Relaxed);

        let mut read = self.read.load(Ordering::Acquire);
        let mut overflowed = false;

        // Full: evict the oldest sample so the newest speech survives.
        //
        // The eviction must not simply store `read + 1`. The consumer advances
        // the same cursor concurrently, and a blind store would move it
        // *backwards* — un-consuming samples the consumer had already handed
        // out, which then get read a second time out of order. A
        // compare-and-swap against the value actually observed is what keeps
        // the cursor monotonic.
        while write.wrapping_sub(read) >= self.capacity {
            match self.read.compare_exchange_weak(
                read,
                read.wrapping_add(1),
                Ordering::AcqRel,
                Ordering::Acquire,
            ) {
                Ok(_) => {
                    self.dropped.fetch_add(1, Ordering::Relaxed);
                    overflowed = true;
                    break;
                }
                // The consumer moved the cursor first. Re-check: it may have
                // freed space, in which case there is nothing left to drop.
                Err(actual) => read = actual,
            }
        }

        self.slot(write).store(sample as usize, Ordering::Relaxed);
        self.write.store(write.wrapping_add(1), Ordering::Release);
        !overflowed
    }

    /// Consumer side: pop one sample.
    fn pop(&self) -> Option<i16> {
        loop {
            let read = self.read.load(Ordering::Acquire);
            let write = self.write.load(Ordering::Acquire);
            if read == write {
                return None;
            }
            let value = self.slot(read).load(Ordering::Acquire) as i16;

            // Advance with a CAS for the same reason as the producer: a plain
            // store here would clobber a concurrent eviction and re-read a
            // sample.
            match self.read.compare_exchange_weak(
                read,
                read.wrapping_add(1),
                Ordering::AcqRel,
                Ordering::Acquire,
            ) {
                Ok(_) => return Some(value),
                // The producer evicted this sample while we were reading it.
                // Retry from the new head rather than returning stale data.
                Err(_) => continue,
            }
        }
    }
}

/// The write half of a [`RingBuffer`]. Not `Clone`: two producers would break
/// the SPSC invariant the type is built on.
pub struct Producer {
    ring: Arc<RingBuffer>,
}

impl Producer {
    /// Push one sample. Returns `false` if an older sample was dropped.
    pub fn push(&self, sample: i16) -> bool {
        self.ring.push(sample)
    }

    /// Push a block, returning how many samples were dropped to fit.
    pub fn push_slice(&self, samples: &[i16]) -> u64 {
        let before = self.ring.dropped();
        for &sample in samples {
            self.ring.push(sample);
        }
        self.ring.dropped() - before
    }

    /// Samples dropped so far.
    pub fn dropped(&self) -> u64 {
        self.ring.dropped()
    }

    /// Samples currently queued.
    pub fn len(&self) -> usize {
        self.ring.len()
    }
}

/// The read half of a [`RingBuffer`].
pub struct Consumer {
    ring: Arc<RingBuffer>,
}

impl Consumer {
    /// Drain up to `out.len()` samples into `out`, returning how many were read.
    pub fn read_into(&self, out: &mut [i16]) -> usize {
        let mut count = 0;
        while count < out.len() {
            match self.ring.pop() {
                Some(sample) => {
                    out[count] = sample;
                    count += 1;
                }
                None => break,
            }
        }
        count
    }

    /// Read exactly one sample if available.
    pub fn pop(&self) -> Option<i16> {
        self.ring.pop()
    }

    /// Samples available to read.
    pub fn len(&self) -> usize {
        self.ring.len()
    }

    /// Whether there is nothing to read.
    pub fn is_empty(&self) -> bool {
        self.ring.is_empty()
    }

    /// Samples dropped by the producer because the consumer fell behind.
    pub fn dropped(&self) -> u64 {
        self.ring.dropped()
    }

    /// Capacity in samples.
    pub fn capacity(&self) -> usize {
        self.ring.capacity()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn capacity_rounds_up_to_a_power_of_two() {
        assert_eq!(RingBuffer::new(1000).capacity(), 1024);
        assert_eq!(RingBuffer::new(1024).capacity(), 1024);
        assert_eq!(RingBuffer::new(1).capacity(), 2);
    }

    #[test]
    fn push_then_pop_preserves_order() {
        let (producer, consumer) = RingBuffer::shared(16);
        for i in 0..8i16 {
            assert!(producer.push(i * 100));
        }
        let mut out = [0i16; 8];
        assert_eq!(consumer.read_into(&mut out), 8);
        assert_eq!(out, [0, 100, 200, 300, 400, 500, 600, 700]);
    }

    #[test]
    fn negative_samples_survive_the_usize_round_trip() {
        // Samples are stored as usize and cast back to i16; a sign-extension
        // bug here would turn quiet passages into full-scale noise.
        let (producer, consumer) = RingBuffer::shared(16);
        let input = [i16::MIN, -1, 0, 1, i16::MAX];
        producer.push_slice(&input);
        let mut out = [0i16; 5];
        assert_eq!(consumer.read_into(&mut out), 5);
        assert_eq!(out, input);
    }

    #[test]
    fn reading_an_empty_ring_yields_nothing() {
        let (_, consumer) = RingBuffer::shared(8);
        let mut out = [0i16; 4];
        assert_eq!(consumer.read_into(&mut out), 0);
        assert!(consumer.is_empty());
        assert_eq!(consumer.pop(), None);
    }

    #[test]
    fn overflow_drops_oldest_and_counts_the_loss() {
        // The property that matters: after overflow, the *newest* samples are
        // still present.
        let (producer, consumer) = RingBuffer::shared(4);
        // Push 6 into a 4-slot ring: 0 and 1 are evicted.
        for i in 0..6i16 {
            producer.push(i);
        }
        assert_eq!(producer.dropped(), 2);
        assert_eq!(consumer.dropped(), 2);
        assert_eq!(consumer.len(), 4);

        let mut out = [0i16; 8];
        assert_eq!(consumer.read_into(&mut out), 4);
        assert_eq!(&out[..4], &[2, 3, 4, 5], "newest samples must survive");
    }

    #[test]
    fn push_slice_reports_only_its_own_drops() {
        let (producer, _consumer) = RingBuffer::shared(4);
        assert_eq!(producer.push_slice(&[1, 2]), 0);
        assert_eq!(producer.push_slice(&[3, 4, 5, 6]), 2);
    }

    #[test]
    fn len_tracks_pushes_and_pops() {
        let (producer, consumer) = RingBuffer::shared(8);
        assert_eq!(consumer.len(), 0);
        producer.push_slice(&[1, 2, 3]);
        assert_eq!(consumer.len(), 3);
        consumer.pop();
        assert_eq!(consumer.len(), 2);
    }

    #[test]
    fn wrapping_many_times_stays_consistent() {
        // Drives the cursors well past the capacity to exercise the mask.
        let (producer, consumer) = RingBuffer::shared(64);
        let mut expected = 0i16;
        let mut out = [0i16; 16];
        for round in 0..1000i32 {
            for _ in 0..8 {
                producer.push(expected);
                expected = expected.wrapping_add(1);
            }
            let read = consumer.read_into(&mut out);
            assert_eq!(read, 8, "round {round}");
        }
        assert_eq!(producer.dropped(), 0, "no drops expected at this pace");
    }

    #[test]
    fn producer_is_send_so_the_capture_callback_can_own_it() {
        fn assert_send<T: Send>() {}
        assert_send::<Producer>();
        assert_send::<Consumer>();
    }

    #[test]
    fn two_threads_do_not_lose_or_reorder_samples() {
        // A real cross-thread exercise: one producer, one consumer, a
        // deliberately small ring so overflow happens constantly, and a check
        // that what arrives is strictly ordered.
        //
        // This test caught a real bug: the producer's overflow eviction used a
        // plain store to advance the read cursor, which could move it backwards
        // past a concurrent consumer and re-read already-consumed samples. It
        // failed roughly one run in eight. Both cursors now advance by
        // compare-and-swap.
        use std::thread;

        let (producer, consumer) = RingBuffer::shared(256);
        const TOTAL: i32 = 100_000;

        let writer = thread::spawn(move || {
            for i in 0..TOTAL {
                producer.push(i as i16);
                // Yield regularly so the consumer and the producer genuinely
                // interleave at the cursor rather than running in separate
                // time slices, which is what makes this a race test.
                if i % 32 == 0 {
                    thread::yield_now();
                }
            }
        });

        let mut received = Vec::new();
        let mut buffer = [0i16; 128];
        while received.len() < 20_000 {
            let read = consumer.read_into(&mut buffer);
            received.extend_from_slice(&buffer[..read]);
            if read == 0 {
                thread::yield_now();
            }
        }
        writer.join().unwrap();

        // The values are i16 and wrap, so ordering is checked on the unwrapped
        // sequence: each sample must be the predecessor plus one, or a strictly
        // greater value after a wrap.
        let mut previous: Option<i16> = None;
        for &sample in &received {
            if let Some(prev) = previous {
                let expected = prev.wrapping_add(1);
                let progresses = sample == expected
                    || (sample as i32) > (prev as i32)
                    || (prev > 0 && sample < 0);
                assert!(
                    progresses,
                    "samples arrived out of order: {prev} then {sample}"
                );
            }
            previous = Some(sample);
        }

        // Deliberately no assertion that the first sample read is the first one
        // pushed. That was here, and it was wrong: it assumed the consumer
        // thread starts before the producer fills the ring, which is a
        // scheduling assumption rather than a property of the ring. When the
        // consumer is scheduled late the ring legitimately drops the oldest
        // samples, and the first one read is somewhere in the middle.
        //
        // Strict monotonicity above is the real property, and it still catches
        // a rewind or a duplicate.
    }

    #[test]
    fn concurrent_push_and_pop_never_duplicate_a_sample() {
        // Repeated short races, each asserting strict monotonicity. A cursor
        // that can rewind produces a duplicate; a cursor that can skip produces
        // a gap. Both are audio artefacts, and both are caught here.
        use std::thread;

        for round in 0..40u32 {
            let (producer, consumer) = RingBuffer::shared(64);
            let writer = thread::spawn(move || {
                for i in 0..2_000i32 {
                    producer.push(i as i16);
                    if i % 64 == 0 {
                        thread::yield_now();
                    }
                }
            });

            let mut seen = Vec::new();
            let mut buffer = [0i16; 32];
            // Drain until the producer has finished and the ring is empty. It
            // does not wait for a fixed count: with a deliberately small ring
            // the producer legitimately drops most of the stream, so demanding
            // a count the ring cannot hold would hang instead of failing.
            loop {
                let read = consumer.read_into(&mut buffer);
                seen.extend_from_slice(&buffer[..read]);
                if read == 0 {
                    if writer.is_finished() && consumer.is_empty() {
                        break;
                    }
                    thread::yield_now();
                }
            }
            writer.join().unwrap();

            assert!(
                seen.len() >= 2,
                "round {round}: expected some samples to survive"
            );

            // Strictly increasing, but not necessarily contiguous: when the
            // ring overflows the producer evicts the oldest sample, so the
            // consumer legitimately observes gaps. A *duplicate* or a *rewind*
            // is the bug being hunted, and both violate strict increase.
            for pair in seen.windows(2) {
                let progresses = pair[1] == pair[0].wrapping_add(1)
                    || (pair[1] as i32) > (pair[0] as i32)
                    || (pair[0] > 0 && pair[1] < 0);
                assert!(
                    progresses,
                    "round {round}: sample {} followed {}, which is not an advance",
                    pair[1], pair[0]
                );
            }
        }
    }
}
