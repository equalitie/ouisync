use metrics::{Counter, Gauge, Histogram, Key, KeyName, Metadata, Recorder, SharedString, Unit};
use std::sync::Arc;

/// Wrapper for `Arc<dyn Recorder>` which itself implement `Recorder`
// TODO: Consider creating a PR upstread that implements `Recorder` for `Arc<impl Recorder>`.
#[derive(Clone)]
pub(crate) struct ArcRecorder(Arc<dyn Recorder + Send + Sync + 'static>);

impl ArcRecorder {
    pub fn new<R>(inner: R) -> Self
    where
        R: Recorder + Send + Sync + 'static,
    {
        Self(Arc::new(inner))
    }
}

impl Recorder for ArcRecorder {
    fn describe_counter(&self, key: KeyName, unit: Option<Unit>, description: SharedString) {
        self.0.describe_counter(key, unit, description)
    }

    fn describe_gauge(&self, key: KeyName, unit: Option<Unit>, description: SharedString) {
        self.0.describe_gauge(key, unit, description)
    }

    fn describe_histogram(&self, key: KeyName, unit: Option<Unit>, description: SharedString) {
        self.0.describe_histogram(key, unit, description)
    }

    fn register_counter(&self, key: &Key, metadata: &Metadata<'_>) -> Counter {
        self.0.register_counter(key, metadata)
    }

    fn register_gauge(&self, key: &Key, metadata: &Metadata<'_>) -> Gauge {
        self.0.register_gauge(key, metadata)
    }

    fn register_histogram(&self, key: &Key, metadata: &Metadata<'_>) -> Histogram {
        self.0.register_histogram(key, metadata)
    }
}
