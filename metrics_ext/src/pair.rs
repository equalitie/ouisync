use metrics::{
    Counter, CounterFn, Gauge, GaugeFn, Histogram, HistogramFn, Key, KeyName, Metadata, Recorder,
    SharedString, Unit,
};
use std::sync::Arc;

/// Recorder which fans-out to a pair of recorders.
pub struct Pair<R0, R1>(pub R0, pub R1);

impl<R0: Recorder, R1: Recorder> Recorder for Pair<R0, R1> {
    fn describe_counter(&self, key: KeyName, unit: Option<Unit>, description: SharedString) {
        self.0
            .describe_counter(key.clone(), unit, description.clone());
        self.1.describe_counter(key, unit, description);
    }

    fn describe_gauge(&self, key: KeyName, unit: Option<Unit>, description: SharedString) {
        self.0
            .describe_gauge(key.clone(), unit, description.clone());
        self.1.describe_gauge(key, unit, description);
    }

    fn describe_histogram(&self, key: KeyName, unit: Option<Unit>, description: SharedString) {
        self.0
            .describe_histogram(key.clone(), unit, description.clone());
        self.1.describe_histogram(key, unit, description);
    }

    fn register_counter(&self, key: &Key, metadata: &Metadata<'_>) -> Counter {
        let c0 = self.0.register_counter(key, metadata);
        let c1 = self.1.register_counter(key, metadata);

        Counter::from_arc(Arc::new(PairCounter(c0, c1)))
    }

    fn register_gauge(&self, key: &Key, metadata: &Metadata<'_>) -> Gauge {
        let g0 = self.0.register_gauge(key, metadata);
        let g1 = self.1.register_gauge(key, metadata);

        Gauge::from_arc(Arc::new(PairGauge(g0, g1)))
    }

    fn register_histogram(&self, key: &Key, metadata: &Metadata<'_>) -> Histogram {
        let h0 = self.0.register_histogram(key, metadata);
        let h1 = self.1.register_histogram(key, metadata);

        Histogram::from_arc(Arc::new(PairHistogram(h0, h1)))
    }
}

struct PairCounter(Counter, Counter);

impl CounterFn for PairCounter {
    fn increment(&self, value: u64) {
        self.0.increment(value);
        self.1.increment(value);
    }

    fn absolute(&self, value: u64) {
        self.0.absolute(value);
        self.1.absolute(value);
    }
}

struct PairGauge(Gauge, Gauge);

impl GaugeFn for PairGauge {
    fn increment(&self, value: f64) {
        self.0.increment(value);
        self.1.increment(value);
    }

    fn decrement(&self, value: f64) {
        self.0.decrement(value);
        self.1.decrement(value);
    }

    fn set(&self, value: f64) {
        self.0.set(value);
        self.1.set(value);
    }
}

struct PairHistogram(Histogram, Histogram);

impl HistogramFn for PairHistogram {
    fn record(&self, value: f64) {
        self.0.record(value);
        self.1.record(value);
    }
}
