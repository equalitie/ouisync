use async_speed_limit::{clock::StandardClock, limiter};
use net::bus::{BusRecvStream, BusSendStream};

type Limiter = limiter::Limiter<StandardClock>;

#[derive(Clone)]
pub(crate) struct Throttle {
    writing: Limiter,
    reading: Limiter,
}

impl Throttle {
    pub(crate) fn new_no_limits() -> Self {
        Self {
            writing: Limiter::new(f64::INFINITY),
            reading: Limiter::new(f64::INFINITY),
        }
    }

    pub(crate) fn set_write_speed_limit(&self, speed_limit: f64) {
        self.writing.set_speed_limit(speed_limit)
    }

    pub(crate) fn set_read_speed_limit(&self, speed_limit: f64) {
        self.reading.set_speed_limit(speed_limit)
    }

    pub(crate) fn limit_writer(&self, writer: BusSendStream) -> ThrottledBusSendStream {
        self.clone().writing.limit(writer)
    }

    pub(crate) fn limit_reader(&self, reader: BusRecvStream) -> ThrottledBusRecvStream {
        self.clone().reading.limit(reader)
    }
}

pub(crate) type ThrottledBusSendStream = limiter::Resource<BusSendStream, StandardClock>;
pub(crate) type ThrottledBusRecvStream = limiter::Resource<BusRecvStream, StandardClock>;
