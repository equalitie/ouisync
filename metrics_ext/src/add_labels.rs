use metrics::{
    Counter, Gauge, Histogram, Key, KeyName, Label, Metadata, Recorder, SharedString, Unit,
};

/// Adds labels to every metric key.
pub struct AddLabels<R> {
    labels: Vec<Label>,
    inner: R,
}

impl<R> AddLabels<R> {
    pub fn new(labels: Vec<Label>, inner: R) -> Self {
        Self { labels, inner }
    }

    fn add_labels(&self, key: &Key) -> Key {
        key.with_extra_labels(self.labels.clone())
    }
}

impl<R: Recorder> Recorder for AddLabels<R> {
    fn describe_counter(&self, key: KeyName, unit: Option<Unit>, description: SharedString) {
        self.inner.describe_counter(key, unit, description)
    }

    fn describe_gauge(&self, key: KeyName, unit: Option<Unit>, description: SharedString) {
        self.inner.describe_gauge(key, unit, description)
    }

    fn describe_histogram(&self, key: KeyName, unit: Option<Unit>, description: SharedString) {
        self.inner.describe_histogram(key, unit, description)
    }

    fn register_counter(&self, key: &Key, metadata: &Metadata<'_>) -> Counter {
        let key = self.add_labels(key);
        self.inner.register_counter(&key, metadata)
    }

    fn register_gauge(&self, key: &Key, metadata: &Metadata<'_>) -> Gauge {
        let key = self.add_labels(key);
        self.inner.register_gauge(&key, metadata)
    }

    fn register_histogram(&self, key: &Key, metadata: &Metadata<'_>) -> Histogram {
        let key = self.add_labels(key);
        self.inner.register_histogram(&key, metadata)
    }
}
