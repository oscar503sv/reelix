//! Live pipeline statistics: frames into the encode queue, frames out of the
//! encoder, frames the (leaky) queue had to drop, and per-frame encoding
//! latency (queue entry → encoder output, matched by timestamp).
//!
//! Frames dropped earlier on purpose (`videorate max-rate`) are not counted:
//! only frames that reached the encode queue and never got encoded are.

use std::collections::VecDeque;
use std::sync::Mutex;
use std::sync::atomic::{AtomicU64, Ordering};
use std::time::{Duration, Instant};

use gst::prelude::*;

use crate::error::{AppError, Result};

/// Frames waiting between the queue and the encoder; more than this means
/// the matching is lost (should never happen with a 5-buffer queue).
const MAX_IN_FLIGHT: usize = 256;

#[derive(Default)]
pub struct PipelineStats {
    captured: AtomicU64,
    encoded: AtomicU64,
    dropped: AtomicU64,
    latency_total_us: AtomicU64,
    latency_max_us: AtomicU64,
    in_flight: Mutex<VecDeque<(u64, Instant)>>,
}

#[derive(Debug, Clone, Copy, PartialEq, Default)]
pub struct StatsSnapshot {
    pub captured: u64,
    pub encoded: u64,
    pub dropped: u64,
    pub avg_latency: Duration,
    pub max_latency: Duration,
}

impl StatsSnapshot {
    /// Share of frames that reached the encoder queue but were dropped.
    pub fn drop_ratio(&self) -> f64 {
        if self.captured == 0 {
            0.0
        } else {
            self.dropped as f64 / self.captured as f64
        }
    }
}

impl PipelineStats {
    /// A frame entered the encode queue.
    pub fn frame_queued(&self, pts: u64, now: Instant) {
        self.captured.fetch_add(1, Ordering::Relaxed);
        if let Ok(mut in_flight) = self.in_flight.lock() {
            if in_flight.len() >= MAX_IN_FLIGHT {
                in_flight.pop_front();
            }
            in_flight.push_back((pts, now));
        }
    }

    /// A frame left the encoder. Earlier queued frames that never came out
    /// were dropped by the leaky queue.
    pub fn frame_encoded(&self, pts: u64, now: Instant) {
        self.encoded.fetch_add(1, Ordering::Relaxed);
        let Ok(mut in_flight) = self.in_flight.lock() else {
            return;
        };
        while let Some(&(queued_pts, queued_at)) = in_flight.front() {
            if queued_pts < pts {
                in_flight.pop_front();
                self.dropped.fetch_add(1, Ordering::Relaxed);
                continue;
            }
            if queued_pts == pts {
                in_flight.pop_front();
                let us =
                    u64::try_from(now.duration_since(queued_at).as_micros()).unwrap_or(u64::MAX);
                self.latency_total_us.fetch_add(us, Ordering::Relaxed);
                self.latency_max_us.fetch_max(us, Ordering::Relaxed);
            }
            break;
        }
    }

    pub fn snapshot(&self) -> StatsSnapshot {
        let encoded = self.encoded.load(Ordering::Relaxed);
        let total = self.latency_total_us.load(Ordering::Relaxed);
        StatsSnapshot {
            captured: self.captured.load(Ordering::Relaxed),
            encoded,
            dropped: self.dropped.load(Ordering::Relaxed),
            avg_latency: Duration::from_micros(total.checked_div(encoded).unwrap_or(0)),
            max_latency: Duration::from_micros(self.latency_max_us.load(Ordering::Relaxed)),
        }
    }

    /// Installs the counting probes.
    pub fn attach(
        self: &std::sync::Arc<Self>,
        queue: &gst::Element,
        encoder: &gst::Element,
    ) -> Result<()> {
        let pad = |element: &gst::Element, name: &str| {
            element
                .static_pad(name)
                .ok_or_else(|| AppError::Pipeline(format!("stats: missing {name} pad")))
        };
        let stats = std::sync::Arc::clone(self);
        pad(queue, "sink")?
            .add_probe(gst::PadProbeType::BUFFER, move |_, info| {
                if let Some(pts) = info.buffer().and_then(|b| b.pts()) {
                    stats.frame_queued(pts.nseconds(), Instant::now());
                }
                gst::PadProbeReturn::Ok
            })
            .ok_or_else(|| AppError::Pipeline("stats: probe failed".into()))?;
        let stats = std::sync::Arc::clone(self);
        pad(encoder, "src")?
            .add_probe(gst::PadProbeType::BUFFER, move |_, info| {
                if let Some(pts) = info.buffer().and_then(|b| b.pts()) {
                    stats.frame_encoded(pts.nseconds(), Instant::now());
                }
                gst::PadProbeReturn::Ok
            })
            .ok_or_else(|| AppError::Pipeline("stats: probe failed".into()))?;
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn counts_latency_and_drops() {
        let stats = PipelineStats::default();
        let t0 = Instant::now();
        let ms = |n| t0 + Duration::from_millis(n);
        stats.frame_queued(1, ms(0));
        stats.frame_queued(2, ms(10)); // dropped by the queue
        stats.frame_queued(3, ms(20));
        stats.frame_encoded(1, ms(15)); // 15 ms
        stats.frame_encoded(3, ms(25)); // 5 ms; frame 2 never came out
        let s = stats.snapshot();
        assert_eq!((s.captured, s.encoded, s.dropped), (3, 2, 1));
        assert_eq!(s.max_latency, Duration::from_millis(15));
        assert_eq!(s.avg_latency, Duration::from_millis(10));
        assert!((s.drop_ratio() - 1.0 / 3.0).abs() < 1e-9);
    }

    #[test]
    fn empty_stats_are_zero() {
        let s = PipelineStats::default().snapshot();
        assert_eq!(s, StatsSnapshot::default());
        assert_eq!(s.drop_ratio(), 0.0);
    }
}
