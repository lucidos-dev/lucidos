//! Where a slow relayed request spent its time, on both gateways.
//!
//! Measurement only (`docs/plans/2026-11-19-gateway-relay-request-queue.md`,
//! Phase 1). Nothing here changes how a request is served. A line names no
//! workspace, gateway or path: a route appears as a hash keyed per process,
//! so lines from one route still group.

use std::collections::hash_map::Entry;
use std::collections::HashMap;
use std::fmt;
use std::hash::{BuildHasher, RandomState};
use std::net::SocketAddr;
use std::pin::Pin;
use std::sync::{Arc, Mutex};
use std::task::{Context, Poll};
use std::time::{Duration, Instant};

use axum::body::{Body, Bytes, HttpBody};
use axum::http::Version;
use axum::response::Response;
use lucidos_diagnostic_log::{line, Line, LogValue};

/// A relayed request slower than this writes one line.
pub const SLOW_RELAY: Duration = Duration::from_millis(200);

/// A stage's end, measured from the request's arrival, or `-` when the request
/// never reached it.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct StageMs(Option<Duration>);

impl fmt::Display for StageMs {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self.0 {
            Some(d) => write!(f, "{}ms", d.as_millis()),
            None => f.write_str("-"),
        }
    }
}
impl LogValue for StageMs {}

/// A fixed word in a line, such as a request kind or an HTTP version.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Label(pub &'static str);

impl fmt::Display for Label {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(self.0)
    }
}
impl LogValue for Label {}

fn http_version(version: Version) -> Label {
    Label(match version {
        Version::HTTP_09 => "0.9",
        Version::HTTP_10 => "1.0",
        Version::HTTP_11 => "1.1",
        Version::HTTP_2 => "2",
        Version::HTTP_3 => "3",
        _ => "?",
    })
}

/// A's stage clock for one relayed request. `kind` says what A did once both
/// probes passed: `shell`, `data`, or `-` when it never got that far.
#[derive(Clone, Debug)]
pub struct RelayClock {
    arrived: Instant,
    version: Label,
    /// The client's peer port: one key per browser connection.
    port: Option<u16>,
    pub kind: Label,
    pub health: StageMs,
    pub contract: StageMs,
    first_byte: StageMs,
}

impl RelayClock {
    pub fn start(version: Version, port: Option<u16>) -> Self {
        RelayClock {
            arrived: Instant::now(),
            version: http_version(version),
            port,
            kind: Label("-"),
            health: StageMs::default(),
            contract: StageMs::default(),
            first_byte: StageMs::default(),
        }
    }

    /// The time since arrival, to store as a stage's end.
    pub fn mark(&self) -> StageMs {
        StageMs(Some(self.arrived.elapsed()))
    }
}

/// A's line for one slow relayed request. `end` is the response end, or `-`
/// for an event stream, whose line fires at first byte.
pub fn slow_relay_line(
    route: u64,
    clock: &RelayClock,
    total: Duration,
    end: StageMs,
    summary: TrafficSummary,
) -> Line {
    line!(
        "[Gateway] slow relay route={} total={} health={} contract={} first_byte={} end={} kind={} http={} conn={} open_conns={} streams={}",
        route,
        StageMs(Some(total)),
        clock.health,
        clock.contract,
        clock.first_byte,
        end,
        clock.kind,
        clock.version,
        clock.port.unwrap_or_default(),
        summary.connections,
        summary.streams
    )
}

/// B's stage clock for one request a paired gateway relayed.
#[derive(Clone, Debug)]
pub struct ReceiveClock {
    arrived: Instant,
    pub verify: StageMs,
    pub contract: StageMs,
    pub engine_first_byte: StageMs,
}

impl ReceiveClock {
    pub fn start() -> Self {
        ReceiveClock {
            arrived: Instant::now(),
            verify: StageMs::default(),
            contract: StageMs::default(),
            engine_first_byte: StageMs::default(),
        }
    }

    /// The time since arrival, to store as a stage's end.
    pub fn mark(&self) -> StageMs {
        StageMs(Some(self.arrived.elapsed()))
    }

    /// Write B's line when the request was slow, and hand `response` back.
    pub fn finish(self, response: Response) -> Response {
        let total = self.arrived.elapsed();
        if total > SLOW_RELAY {
            crate::diagnostic_log::typed(self.slow_line(total));
        }
        response
    }

    fn slow_line(&self, total: Duration) -> Line {
        line!(
            "[Gateway] slow relayed receive total={} verify={} contract={} engine_first_byte={}",
            StageMs(Some(total)),
            self.verify,
            self.contract,
            self.engine_first_byte
        )
    }
}

/// Relayed requests in flight on A, per route.
#[derive(Default)]
pub struct RelayTraffic {
    routes: Mutex<HashMap<String, RouteTraffic>>,
    /// Keys the route numbers, so a number names no route outside this process.
    numbers: RandomState,
}

#[derive(Default)]
struct RouteTraffic {
    /// In-flight requests per client peer address.
    peers: HashMap<SocketAddr, usize>,
    in_flight: usize,
    streams: usize,
}

/// One route's traffic at one moment.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct TrafficSummary {
    /// Distinct client connections with a relayed request in flight.
    pub connections: usize,
    /// Relayed event streams held open.
    pub streams: usize,
}

impl RouteTraffic {
    fn summary(&self) -> TrafficSummary {
        TrafficSummary {
            connections: self.peers.len(),
            streams: self.streams,
        }
    }
}

impl RelayTraffic {
    /// Count a relayed request on `route` until the returned guard drops.
    pub fn enter(self: &Arc<Self>, route: &str, peer: Option<SocketAddr>) -> InFlight {
        let mut routes = self.routes.lock().unwrap_or_else(|e| e.into_inner());
        let traffic = routes.entry(route.to_string()).or_default();
        traffic.in_flight += 1;
        if let Some(peer) = peer {
            *traffic.peers.entry(peer).or_default() += 1;
        }
        InFlight {
            traffic: self.clone(),
            route: route.to_string(),
            number: self.numbers.hash_one(route) % 1_000_000,
            peer,
            stream: false,
        }
    }

    pub fn summary(&self, route: &str) -> TrafficSummary {
        self.with_route(route, |t| t.summary()).unwrap_or_default()
    }

    fn with_route<T>(&self, route: &str, f: impl FnOnce(&mut RouteTraffic) -> T) -> Option<T> {
        let mut routes = self.routes.lock().unwrap_or_else(|e| e.into_inner());
        routes.get_mut(route).map(f)
    }

    fn leave(&self, flight: &InFlight) -> Option<TrafficSummary> {
        let mut routes = self.routes.lock().unwrap_or_else(|e| e.into_inner());
        let traffic = routes.get_mut(&flight.route)?;
        traffic.in_flight -= 1;
        if let Some(Entry::Occupied(mut count)) = flight.peer.map(|p| traffic.peers.entry(p)) {
            *count.get_mut() -= 1;
            if *count.get() == 0 {
                count.remove();
            }
        }
        if flight.stream {
            traffic.streams -= 1;
        }
        let summary = traffic.summary();
        if traffic.in_flight == 0 {
            routes.remove(&flight.route);
        }
        Some(summary)
    }
}

/// One relayed request, counted on its route until it drops.
pub struct InFlight {
    traffic: Arc<RelayTraffic>,
    route: String,
    number: u64,
    peer: Option<SocketAddr>,
    stream: bool,
}

impl InFlight {
    pub fn summary(&self) -> TrafficSummary {
        self.traffic.summary(&self.route)
    }

    /// Count this request as an open event stream, and log the new count.
    pub fn hold_stream(&mut self) {
        if self.stream {
            return;
        }
        self.stream = true;
        let summary = self.traffic.with_route(&self.route, |t| {
            t.streams += 1;
            t.summary()
        });
        if let Some(summary) = summary {
            crate::diagnostic_log::typed(stream_line(self.number, true, summary));
        }
    }
}

impl Drop for InFlight {
    fn drop(&mut self) {
        let summary = self.traffic.leave(self);
        if let (true, Some(summary)) = (self.stream, summary) {
            crate::diagnostic_log::typed(stream_line(self.number, false, summary));
        }
    }
}

fn stream_line(route: u64, opened: bool, summary: TrafficSummary) -> Line {
    line!(
        "[Gateway] relay event stream {} route={} open_conns={} streams={}",
        Label(if opened { "opened" } else { "closed" }),
        route,
        summary.connections,
        summary.streams
    )
}

/// `response` with its body timed: A's line, when slow, fires as the body
/// ends or drops. An event stream's line fires now, at first byte.
pub fn timed_response(mut clock: RelayClock, mut flight: InFlight, response: Response) -> Response {
    clock.first_byte = clock.mark();
    let event_stream = response
        .headers()
        .get(axum::http::header::CONTENT_TYPE)
        .and_then(|v| v.to_str().ok())
        .is_some_and(|v| v.trim_start().starts_with("text/event-stream"));
    if event_stream {
        flight.hold_stream();
        let total = clock.arrived.elapsed();
        if total > SLOW_RELAY {
            let line = slow_relay_line(
                flight.number,
                &clock,
                total,
                StageMs::default(),
                flight.summary(),
            );
            crate::diagnostic_log::typed(line);
        }
    }
    let clock = (!event_stream).then_some(clock);
    response.map(|body| {
        Body::new(Timed {
            body,
            finish: Some(Finish { clock, flight }),
        })
    })
}

/// Releases the request's count as it drops. With a clock, it first writes
/// the line a slow request owes. An event stream has none: its line fired at
/// first byte.
struct Finish {
    clock: Option<RelayClock>,
    flight: InFlight,
}

impl Drop for Finish {
    fn drop(&mut self) {
        let Some(clock) = &self.clock else {
            return;
        };
        let total = clock.arrived.elapsed();
        if total > SLOW_RELAY {
            let end = StageMs(Some(total));
            let line =
                slow_relay_line(self.flight.number, clock, total, end, self.flight.summary());
            crate::diagnostic_log::typed(line);
        }
    }
}

/// A body that runs `finish` once it ends or drops. Size hint and end of
/// stream pass through, so the response frames as it did untimed.
struct Timed {
    body: Body,
    finish: Option<Finish>,
}

impl HttpBody for Timed {
    type Data = Bytes;
    type Error = axum::Error;

    fn poll_frame(
        mut self: Pin<&mut Self>,
        cx: &mut Context<'_>,
    ) -> Poll<Option<Result<http_body::Frame<Bytes>, axum::Error>>> {
        let polled = Pin::new(&mut self.body).poll_frame(cx);
        if matches!(polled, Poll::Ready(None)) {
            self.finish.take();
        }
        polled
    }

    fn is_end_stream(&self) -> bool {
        self.body.is_end_stream()
    }

    fn size_hint(&self) -> http_body::SizeHint {
        self.body.size_hint()
    }
}

#[cfg(test)]
#[path = "relay_timing_tests.rs"]
mod tests;
