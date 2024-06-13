use hdrhistogram::{sync::Recorder, Histogram, SyncHistogram};
use ouisync::network::{Network, TrafficStats};
use serde::{ser::SerializeMap, Serialize};
use std::{
    mem,
    time::{Duration, Instant},
};

pub(crate) struct SummaryRecorder {
    send: SyncHistogram<u64>,
    recv: SyncHistogram<u64>,
    start: Instant,
}

impl SummaryRecorder {
    pub fn new() -> Self {
        let send = SyncHistogram::from(Histogram::<u64>::new(3).unwrap());
        let recv = SyncHistogram::from(Histogram::<u64>::new(3).unwrap());

        Self {
            send,
            recv,
            start: Instant::now(),
        }
    }

    pub fn actor(&self) -> ActorSummaryRecorder {
        ActorSummaryRecorder {
            send: self.send.recorder(),
            recv: self.recv.recorder(),
        }
    }

    pub fn finalize(mut self, label: String) -> Summary {
        self.send.refresh();
        self.recv.refresh();

        Summary {
            label,
            duration: self.start.elapsed(),
            send: mem::replace(&mut self.send, Histogram::new(3).unwrap()),
            recv: mem::replace(&mut self.recv, Histogram::new(3).unwrap()),
        }
    }
}

pub(crate) struct ActorSummaryRecorder {
    send: Recorder<u64>,
    recv: Recorder<u64>,
}

impl ActorSummaryRecorder {
    pub fn record(mut self, network: &Network) {
        let TrafficStats { send, recv, .. } = network.traffic_stats();

        info!(send, recv);

        self.send.record(send).unwrap();
        self.recv.record(recv).unwrap();
    }
}

#[derive(Serialize)]
pub(crate) struct Summary {
    #[serde(skip_serializing_if = "String::is_empty")]
    pub label: String,
    #[serde(serialize_with = "serialize_duration")]
    pub duration: Duration,
    #[serde(serialize_with = "serialize_histogram")]
    pub send: Histogram<u64>,
    #[serde(serialize_with = "serialize_histogram")]
    pub recv: Histogram<u64>,
}

fn serialize_duration<S>(value: &Duration, s: S) -> Result<S::Ok, S::Error>
where
    S: serde::Serializer,
{
    value.as_secs_f64().serialize(s)
}

fn serialize_histogram<S>(value: &Histogram<u64>, s: S) -> Result<S::Ok, S::Error>
where
    S: serde::Serializer,
{
    let mut s = s.serialize_map(Some(4))?;
    s.serialize_entry("min", &value.min())?;
    s.serialize_entry("max", &value.max())?;
    s.serialize_entry("mean", &(value.mean().round() as u64))?;
    s.serialize_entry("stdev", &(value.stdev().round() as u64))?;
    s.end()
}
