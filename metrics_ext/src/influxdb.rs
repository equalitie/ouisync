use metrics::{
    Counter, CounterFn, Gauge, GaugeFn, Histogram, HistogramFn, Key, KeyName, Label, Metadata,
    Recorder, SharedString, Unit,
};
use metrics_util::storage::Summary;
use reqwest::{
    header::{self, HeaderMap, HeaderValue},
    Client, ClientBuilder, StatusCode,
};
use std::{
    collections::HashMap,
    fmt::Write,
    sync::Arc,
    time::{Duration, SystemTime},
};
use tokio::sync::mpsc;

const BUFFER_SIZE: usize = 1024;
const MAX_REQUEST_LINES: usize = 1024;
const QUANTILES: &[f64] = &[0.0, 0.5, 0.9, 0.99, 1.0];

/// Exports metrics to InfuxDB
pub struct InfluxDbRecorder {
    tx: mpsc::Sender<Event>,
}

impl InfluxDbRecorder {
    pub fn new(params: InfluxDbParams) -> (Self, impl Future<Output = ()> + Send + 'static) {
        let (tx, rx) = mpsc::channel(BUFFER_SIZE);
        (Self { tx }, run(rx, params))
    }
}

impl Recorder for InfluxDbRecorder {
    fn describe_counter(&self, _key: KeyName, _unit: Option<Unit>, _description: SharedString) {}

    fn describe_gauge(&self, _key: KeyName, _unit: Option<Unit>, _description: SharedString) {}

    fn describe_histogram(&self, _key: KeyName, _unit: Option<Unit>, _description: SharedString) {}

    fn register_counter(&self, key: &Key, _metadata: &Metadata<'_>) -> Counter {
        Counter::from_arc(Arc::new(Metric {
            key: key.clone(),
            tx: self.tx.clone(),
        }))
    }

    fn register_gauge(&self, key: &Key, _metadata: &Metadata<'_>) -> Gauge {
        Gauge::from_arc(Arc::new(Metric {
            key: key.clone(),
            tx: self.tx.clone(),
        }))
    }

    fn register_histogram(&self, key: &Key, _metadata: &Metadata<'_>) -> Histogram {
        Histogram::from_arc(Arc::new(Metric {
            key: key.clone(),
            tx: self.tx.clone(),
        }))
    }
}

pub struct InfluxDbParams {
    pub endpoint: String,
    pub token: String,
    pub org: String,
    pub bucket: String,
}

struct Metric {
    key: Key,
    tx: mpsc::Sender<Event>,
}

impl CounterFn for Metric {
    fn increment(&self, value: u64) {
        self.tx
            .try_send(Event::CounterIncrement(
                self.key.clone(),
                value,
                SystemTime::now(),
            ))
            .ok();
    }

    fn absolute(&self, value: u64) {
        self.tx
            .try_send(Event::CounterAbsolute(
                self.key.clone(),
                value,
                SystemTime::now(),
            ))
            .ok();
    }
}

impl GaugeFn for Metric {
    fn increment(&self, value: f64) {
        self.tx
            .try_send(Event::GaugeIncrement(
                self.key.clone(),
                value,
                SystemTime::now(),
            ))
            .ok();
    }

    fn decrement(&self, value: f64) {
        self.tx
            .try_send(Event::GaugeDecrement(
                self.key.clone(),
                value,
                SystemTime::now(),
            ))
            .ok();
    }

    fn set(&self, value: f64) {
        self.tx
            .try_send(Event::GaugeSet(self.key.clone(), value, SystemTime::now()))
            .ok();
    }
}

impl HistogramFn for Metric {
    fn record(&self, value: f64) {
        self.tx
            .try_send(Event::HistogramRecord(
                self.key.clone(),
                value,
                SystemTime::now(),
            ))
            .ok();
    }
}

struct HistogramData {
    summary: Summary,
    sum: f64,
}

impl HistogramData {
    fn new() -> Self {
        Self {
            summary: Summary::with_defaults(),
            sum: 0.0,
        }
    }

    fn record(&mut self, value: f64) {
        self.summary.add(value);
        self.sum += value;
    }

    fn quantile(&self, quantile: f64) -> f64 {
        self.summary.quantile(quantile).unwrap_or(0.0)
    }

    fn count(&self) -> u64 {
        self.summary.count() as u64
    }

    fn sum(&self) -> f64 {
        self.sum
    }
}

enum Event {
    CounterIncrement(Key, u64, SystemTime),
    CounterAbsolute(Key, u64, SystemTime),
    GaugeIncrement(Key, f64, SystemTime),
    GaugeDecrement(Key, f64, SystemTime),
    GaugeSet(Key, f64, SystemTime),
    HistogramRecord(Key, f64, SystemTime),
}

async fn run(mut rx: mpsc::Receiver<Event>, params: InfluxDbParams) {
    let client = match build_client(&params) {
        Ok(client) => client,
        Err(error) => {
            tracing::error!("failed to build http client: {error:?}");
            return;
        }
    };

    let url = build_url(&params);

    #[allow(clippy::mutable_key_type)] // false positive
    let mut counters = HashMap::new();

    #[allow(clippy::mutable_key_type)] // false positive
    let mut gauges = HashMap::new();

    #[allow(clippy::mutable_key_type)] // false positive
    let mut histograms = HashMap::new();

    let mut event_buffer = Vec::new();

    loop {
        if rx.recv_many(&mut event_buffer, MAX_REQUEST_LINES).await == 0 {
            break;
        }

        let mut body = String::new();

        for event in event_buffer.drain(..) {
            match event {
                Event::CounterIncrement(key, value, ts) => {
                    write_header(&mut body, &key);
                    let counter = counters.entry(key).or_default();
                    *counter += value;
                    write_counter(&mut body, *counter, ts);
                }
                Event::CounterAbsolute(key, value, ts) => {
                    write_header(&mut body, &key);
                    let counter = counters.entry(key).or_default();
                    *counter = (*counter).max(value);
                    write_counter(&mut body, *counter, ts);
                }
                Event::GaugeIncrement(key, value, ts) => {
                    write_header(&mut body, &key);
                    let gauge = gauges.entry(key).or_default();
                    *gauge += value;
                    write_gauge(&mut body, *gauge, ts);
                }
                Event::GaugeDecrement(key, value, ts) => {
                    write_header(&mut body, &key);
                    let gauge = gauges.entry(key).or_default();
                    *gauge += value;
                    write_gauge(&mut body, *gauge, ts);
                }
                Event::GaugeSet(key, value, ts) => {
                    write_header(&mut body, &key);
                    let gauge = gauges.entry(key).or_default();
                    *gauge = value;
                    write_gauge(&mut body, *gauge, ts);
                }
                Event::HistogramRecord(key, value, ts) => {
                    write_header(&mut body, &key);
                    let histogram = histograms
                        .entry(key.clone())
                        .or_insert_with(HistogramData::new);
                    histogram.record(value);
                    write_histogram(&mut body, histogram, ts);
                }
            }

            writeln!(&mut body).unwrap();
        }

        match client.post(&url).body(body).send().await {
            Ok(response) if response.status() == StatusCode::NO_CONTENT => (),
            Ok(response) => {
                let status = response.status();
                let body = response.text().await;
                tracing::error!(%status, ?body);
            }
            Err(error) => {
                tracing::error!("failed to send http request: {error:?}");
            }
        }
    }
}

fn build_client(params: &InfluxDbParams) -> reqwest::Result<Client> {
    let mut headers = HeaderMap::new();

    let mut token = HeaderValue::from_str(&format!("Token {}", params.token)).unwrap();
    token.set_sensitive(true);
    headers.insert(header::AUTHORIZATION, token);

    headers.insert(
        header::CONTENT_TYPE,
        HeaderValue::from_static("text/plain; charset=utf-8"),
    );
    headers.insert(header::ACCEPT, HeaderValue::from_static("application/json"));

    ClientBuilder::new().default_headers(headers).build()
}

fn build_url(params: &InfluxDbParams) -> String {
    format!(
        "{}/write?org={}&bucket={}&precision=ms",
        params.endpoint, params.org, params.bucket
    )
}

fn write_header(output: &mut String, key: &Key) {
    write_sanitized(output, key.name());
    write_labels(output, key.labels());
}

fn write_counter(output: &mut String, value: u64, ts: SystemTime) {
    write!(output, " value={}", value).unwrap();
    write_timestamp(output, ts);
}

fn write_gauge(output: &mut String, value: f64, ts: SystemTime) {
    write!(output, " value={}", value).unwrap();
    write_timestamp(output, ts);
}

fn write_histogram(output: &mut String, histogram: &HistogramData, ts: SystemTime) {
    write!(output, " ").unwrap();

    let mut first = true;
    for &quantile in QUANTILES {
        if first {
            first = false;
        } else {
            write!(output, ",").unwrap();
        }

        write!(
            output,
            "quantile[{}]={}",
            quantile,
            histogram.quantile(quantile)
        )
        .unwrap()
    }

    write!(output, ",count={}", histogram.count()).unwrap();
    write!(output, ",sum={}", histogram.sum()).unwrap();

    write_timestamp(output, ts);
}

fn write_sanitized(output: &mut String, name: &str) {
    for c in name.chars() {
        if c.is_whitespace() {
            output.push('_');
        } else {
            output.push(c);
        }
    }
}

fn write_labels<'a, 'b>(output: &'a mut String, labels: impl IntoIterator<Item = &'b Label>) {
    for label in labels {
        write!(output, ",").unwrap();
        write_sanitized(output, label.key());
        write!(output, "=").unwrap();
        write_sanitized(output, label.value());
    }
}

fn write_timestamp(output: &mut String, ts: SystemTime) {
    write!(
        output,
        " {}",
        ts.duration_since(SystemTime::UNIX_EPOCH)
            .unwrap_or(Duration::ZERO)
            .as_millis()
    )
    .unwrap();
}
