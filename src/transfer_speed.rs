//! Bounded, allocation-free sampling for the desktop transfer speed labels.

use std::time::{Duration, Instant};

const WINDOW: Duration = Duration::from_secs(1);
const MIN_MEASUREMENT: Duration = Duration::from_millis(1);
// The fastest selectable 30 ms dwell produces fewer than 34 data frames/sec.
// Spare slots also accommodate UI queues without allocating in the frame path.
const SAMPLE_CAPACITY: usize = 64;

#[derive(Clone, Copy, Debug)]
struct Sample {
    at: Instant,
    bytes: u64,
}

/// Current payload throughput over the preceding second, including idle time.
///
/// Callers supply existing frame timestamps and newly delivered payload bytes;
/// calibration, metadata and duplicate captures contribute no bytes. Samples
/// must arrive in chronological order. Reads expire old samples so a paused or
/// stalled transfer falls to zero without a new capture or a separate timer.
#[derive(Debug)]
pub struct SpeedMeter {
    samples: [Option<Sample>; SAMPLE_CAPACITY],
    oldest: usize,
    len: usize,
    window_bytes: u64,
    started: Option<Instant>,
}

impl Default for SpeedMeter {
    fn default() -> Self {
        Self {
            samples: [None; SAMPLE_CAPACITY],
            oldest: 0,
            len: 0,
            window_bytes: 0,
            started: None,
        }
    }
}

impl SpeedMeter {
    /// Start a new measurement, discarding every previous transfer's samples.
    pub fn reset(&mut self, at: Instant) {
        *self = Self {
            started: Some(at),
            ..Self::default()
        };
    }

    /// Record one newly delivered payload using its original frame timestamp.
    /// Recording does not allocate, read a clock or trigger a UI repaint.
    pub fn record_bytes(&mut self, bytes: u64, at: Instant) {
        if bytes == 0 {
            return;
        }
        self.expire(at);
        self.started.get_or_insert(at);
        self.window_bytes = self.window_bytes.saturating_add(bytes);
        if self.len > 0 {
            let newest = (self.oldest + self.len - 1) % SAMPLE_CAPACITY;
            let sample = self.samples[newest].as_mut().unwrap();
            if sample.at == at {
                sample.bytes = sample.bytes.saturating_add(bytes);
                return;
            }
        }
        if self.len == SAMPLE_CAPACITY {
            // A future source may exceed the present frame rate. Merge the two
            // oldest samples instead of dropping bytes or growing a queue.
            let previous = self.samples[self.oldest].take().unwrap();
            self.oldest = (self.oldest + 1) % SAMPLE_CAPACITY;
            let next = self.samples[self.oldest].as_mut().unwrap();
            next.bytes = next.bytes.saturating_add(previous.bytes);
            self.len -= 1;
        }
        let next = (self.oldest + self.len) % SAMPLE_CAPACITY;
        self.samples[next] = Some(Sample { at, bytes });
        self.len += 1;
    }

    /// Return bytes/second; steady readings use a one-second observation span.
    /// A 1 ms floor keeps zero-length initial observations finite.
    pub fn bytes_per_second(&mut self, now: Instant) -> f64 {
        self.expire(now);
        let Some(started) = self.started else {
            return 0.0;
        };
        let seconds = now
            .saturating_duration_since(started)
            .clamp(MIN_MEASUREMENT, WINDOW)
            .as_secs_f64();
        self.window_bytes as f64 / seconds
    }

    fn expire(&mut self, now: Instant) {
        while self.len > 0 {
            let sample = self.samples[self.oldest].unwrap();
            if now.saturating_duration_since(sample.at) < WINDOW {
                break;
            }
            self.window_bytes = self.window_bytes.saturating_sub(sample.bytes);
            self.samples[self.oldest] = None;
            self.oldest = (self.oldest + 1) % SAMPLE_CAPACITY;
            self.len -= 1;
        }
    }
}

/// Use the same binary size units as the file-size labels on both clients.
pub fn format_speed(bytes_per_second: f64) -> String {
    if !bytes_per_second.is_finite() || bytes_per_second <= 0.0 {
        return "0 B/s".into();
    }
    if bytes_per_second < 1024.0 {
        format!("{bytes_per_second:.0} B/s")
    } else if bytes_per_second < 1024.0 * 1024.0 {
        format!("{:.1} KB/s", bytes_per_second / 1024.0)
    } else if bytes_per_second < 1024.0 * 1024.0 * 1024.0 {
        format!("{:.2} MB/s", bytes_per_second / (1024.0 * 1024.0))
    } else {
        format!("{:.2} GB/s", bytes_per_second / (1024.0 * 1024.0 * 1024.0))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn current_speed_uses_recent_payload_instead_of_transfer_average() {
        let start = Instant::now();
        let mut meter = SpeedMeter::default();
        meter.reset(start);
        for frame in 1..=20 {
            meter.record_bytes(1024, start + Duration::from_millis(frame * 50));
        }
        assert_eq!(meter.bytes_per_second(start + WINDOW), 20.0 * 1024.0);
        // Existing samples decay during the stall without any new frame.
        assert_eq!(
            meter.bytes_per_second(start + Duration::from_millis(1500)),
            10.0 * 1024.0
        );
        assert_eq!(meter.bytes_per_second(start + 2 * WINDOW), 0.0);
        meter.record_bytes(123, start + 3 * WINDOW);
        assert_eq!(meter.bytes_per_second(start + 3 * WINDOW), 123.0);
    }

    #[test]
    fn payload_lengths_and_queued_timestamps_remain_exact() {
        let start = Instant::now();
        let mut meter = SpeedMeter::default();
        meter.reset(start);
        meter.record_bytes(4096, start + Duration::from_millis(100));
        meter.record_bytes(37, start + Duration::from_millis(200));
        // A delayed frontend poll uses frame time, not when it drained the queue.
        assert_eq!(
            meter.bytes_per_second(start + Duration::from_millis(500)),
            (4096.0 + 37.0) / 0.5
        );
        assert_eq!(
            meter.bytes_per_second(start + Duration::from_millis(1150)),
            37.0
        );
    }

    #[test]
    fn reset_and_initial_readings_do_not_keep_previous_transfer_speed() {
        let start = Instant::now();
        let mut meter = SpeedMeter::default();
        assert_eq!(meter.bytes_per_second(start), 0.0);
        meter.record_bytes(0, start);
        assert_eq!(meter.bytes_per_second(start), 0.0);
        meter.reset(start);
        meter.record_bytes(100, start);
        assert_eq!(meter.bytes_per_second(start), 100_000.0);
        meter.reset(start + WINDOW);
        assert_eq!(meter.bytes_per_second(start + WINDOW), 0.0);
        meter.record_bytes(20, start + WINDOW);
        assert_eq!(meter.bytes_per_second(start + WINDOW), 20_000.0);
    }

    #[test]
    fn unusually_dense_samples_keep_bounded_storage_without_losing_bytes() {
        let start = Instant::now();
        let mut meter = SpeedMeter::default();
        meter.reset(start);
        for sample in 1..=1000 {
            meter.record_bytes(1, start + Duration::from_micros(sample));
        }
        assert_eq!(meter.len, SAMPLE_CAPACITY);
        assert_eq!(
            meter.bytes_per_second(start + Duration::from_millis(100)),
            10_000.0
        );
        assert_eq!(meter.bytes_per_second(start + 2 * WINDOW), 0.0);
    }

    #[test]
    fn equal_timestamps_share_a_single_sample() {
        let start = Instant::now();
        let mut meter = SpeedMeter::default();
        meter.reset(start);
        meter.record_bytes(1024, start);
        meter.record_bytes(17, start);
        assert_eq!(meter.len, 1);
        assert_eq!(meter.bytes_per_second(start), 1_041_000.0);
        assert_eq!(meter.bytes_per_second(start + WINDOW), 0.0);
    }

    #[test]
    fn speed_labels_use_consistent_units_and_reject_invalid_numbers() {
        assert_eq!(format_speed(0.0), "0 B/s");
        assert_eq!(format_speed(-1.0), "0 B/s");
        assert_eq!(format_speed(f64::NAN), "0 B/s");
        assert_eq!(format_speed(f64::INFINITY), "0 B/s");
        assert_eq!(format_speed(512.0), "512 B/s");
        assert_eq!(format_speed(1536.0), "1.5 KB/s");
        assert_eq!(format_speed(1.25 * 1024.0 * 1024.0), "1.25 MB/s");
        assert_eq!(format_speed(1024.0 * 1024.0 * 1024.0), "1.00 GB/s");
    }
}
