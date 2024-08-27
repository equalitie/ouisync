use serde::{Deserialize, Serialize};
use std::time::Instant;

/// Helper to track network throughput (number of bytes sent/received per second).
#[derive(Default)]
pub(super) struct ThroughputTracker {
    prev: Option<Sample>,
}

impl ThroughputTracker {
    pub fn new() -> Self {
        Self::default()
    }

    /// Returns the current throughput, given the current total amount of bytes sent and received.
    /// The returned values are in millibytes (1/1000 bytes) per second (this weird unit allows us
    /// to use integer-only arithmetics while retaining reasonable precision).
    ///
    /// Note: For best results, call this in regular intervals (e.g., once per second).
    pub fn sample(&mut self, bytes_send: u64, bytes_recv: u64) -> Throughput {
        let next_timestamp = Instant::now();

        let (prev_throughput, next_throughput) = if let Some(prev) = self.prev.take() {
            let time = next_timestamp - prev.timestamp;

            let next_throughput = if time.is_zero() {
                prev.throughput
            } else {
                let ms = time.as_millis().try_into().unwrap_or(u64::MAX);

                Throughput {
                    send: bytes_send.saturating_sub(prev.bytes_send) / ms,
                    recv: bytes_recv.saturating_sub(prev.bytes_recv) / ms,
                }
            };

            (prev.throughput, next_throughput)
        } else {
            (Throughput::default(), Throughput::default())
        };

        self.prev = Some(Sample {
            timestamp: next_timestamp,
            bytes_send,
            bytes_recv,
            throughput: next_throughput,
        });

        // Rolling average using window of two samples.
        Throughput {
            send: (prev_throughput.send + next_throughput.send) / 2,
            recv: (prev_throughput.recv + next_throughput.recv) / 2,
        }
    }
}

#[derive(Default, Clone, Copy, Eq, PartialEq, Debug, Serialize, Deserialize)]
pub struct Throughput {
    /// Send throughput in millibytes per second
    pub send: u64,
    /// Receive throughput in millibytes per second
    pub recv: u64,
}

struct Sample {
    timestamp: Instant,
    bytes_send: u64,
    bytes_recv: u64,
    throughput: Throughput,
}
