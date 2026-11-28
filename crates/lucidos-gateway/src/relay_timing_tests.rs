use super::*;
use axum::http::header;
use axum::response::IntoResponse;
use lucidos_diagnostic_log::Destination;

const ROUTE: &str = "/private-workspace@private-gateway";

fn peer(port: u16) -> Option<SocketAddr> {
    Some(SocketAddr::from(([127, 0, 0, 1], port)))
}

#[test]
fn a_stage_renders_in_milliseconds_or_as_unreached() {
    assert_eq!(
        StageMs(Some(Duration::from_millis(1234))).to_string(),
        "1234ms"
    );
    assert_eq!(StageMs(None).to_string(), "-");
}

#[test]
fn a_slow_line_carries_every_stage_and_names_no_route() {
    let mut clock = RelayClock::start(Version::HTTP_11, Some(51234));
    clock.health = StageMs(Some(Duration::from_millis(400)));
    clock.contract = StageMs(Some(Duration::from_millis(900)));
    clock.first_byte = StageMs(Some(Duration::from_millis(950)));
    clock.kind = Label("data");
    let summary = TrafficSummary {
        connections: 6,
        streams: 3,
    };
    let line = slow_relay_line(
        2,
        &clock,
        Duration::from_millis(1000),
        StageMs(Some(Duration::from_millis(1000))),
        summary,
    );
    let text = line.render(Destination::Maintainer);
    assert!(
        text.contains(
            "route=2 total=1000ms health=400ms contract=900ms first_byte=950ms end=1000ms \
             kind=data http=1.1 conn=51234 open_conns=6 streams=3"
        ),
        "{text}"
    );
    assert!(!text.contains("private"), "{text}");
}

#[test]
fn an_unsettled_request_renders_its_unreached_stages() {
    let clock = RelayClock::start(Version::HTTP_2, None);
    let text = slow_relay_line(
        1,
        &clock,
        Duration::from_millis(300),
        StageMs(None),
        TrafficSummary::default(),
    )
    .render(Destination::Maintainer);
    assert!(
        text.contains("health=- contract=- first_byte=- end=- kind=- http=2 conn=0"),
        "{text}"
    );
}

#[test]
fn b_renders_its_own_stages() {
    let mut clock = ReceiveClock::start();
    clock.verify = StageMs(Some(Duration::from_millis(1)));
    clock.engine_first_byte = StageMs(Some(Duration::from_millis(250)));
    let text = clock
        .slow_line(Duration::from_millis(250))
        .render(Destination::Maintainer);
    assert!(
        text.contains("total=250ms verify=1ms contract=- engine_first_byte=250ms"),
        "{text}"
    );
}

#[test]
fn connections_count_distinct_peer_ports_in_flight() {
    let traffic = Arc::<RelayTraffic>::default();
    let a = traffic.enter(ROUTE, peer(1000));
    let b = traffic.enter(ROUTE, peer(1000));
    let c = traffic.enter(ROUTE, peer(2000));
    let other_host = traffic.enter(ROUTE, Some(SocketAddr::from(([10, 0, 0, 2], 2000))));
    let unkeyed = traffic.enter(ROUTE, None);
    assert_eq!(traffic.summary(ROUTE).connections, 3);
    drop(a);
    assert_eq!(traffic.summary(ROUTE).connections, 3);
    drop(b);
    assert_eq!(traffic.summary(ROUTE).connections, 2);
    drop((c, other_host, unkeyed));
    assert_eq!(traffic.summary(ROUTE), TrafficSummary::default());
}

#[test]
fn a_held_stream_counts_once_and_releases_on_drop() {
    let traffic = Arc::<RelayTraffic>::default();
    let mut flight = traffic.enter(ROUTE, peer(1000));
    flight.hold_stream();
    flight.hold_stream();
    assert_eq!(traffic.summary(ROUTE).streams, 1);
    drop(flight);
    assert_eq!(traffic.summary(ROUTE).streams, 0);
}

#[test]
fn a_route_keeps_its_number_across_an_idle_gap() {
    let traffic = Arc::<RelayTraffic>::default();
    let first = traffic.enter(ROUTE, None).number;
    assert_eq!(traffic.summary(ROUTE), TrafficSummary::default());
    assert_eq!(traffic.enter(ROUTE, None).number, first);
}

#[tokio::test]
async fn an_event_stream_holds_its_count_until_the_body_drops() {
    let traffic = Arc::<RelayTraffic>::default();
    let response = (
        [(header::CONTENT_TYPE, "text/event-stream")],
        "data: one\n\n",
    )
        .into_response();
    let clock = RelayClock::start(Version::HTTP_11, Some(1000));
    let timed = timed_response(clock, traffic.enter(ROUTE, peer(1000)), response);
    assert_eq!(
        traffic.summary(ROUTE),
        TrafficSummary {
            connections: 1,
            streams: 1
        }
    );
    drop(timed);
    assert_eq!(traffic.summary(ROUTE), TrafficSummary::default());
}

#[tokio::test]
async fn a_timed_body_keeps_its_size_and_releases_at_its_end() {
    let traffic = Arc::<RelayTraffic>::default();
    let clock = RelayClock::start(Version::HTTP_11, Some(1000));
    let timed = timed_response(
        clock,
        traffic.enter(ROUTE, peer(1000)),
        "hello".into_response(),
    );
    assert_eq!(timed.body().size_hint().exact(), Some(5));
    assert_eq!(traffic.summary(ROUTE).connections, 1);
    let bytes = axum::body::to_bytes(timed.into_body(), usize::MAX)
        .await
        .unwrap();
    assert_eq!(&bytes[..], b"hello");
    assert_eq!(traffic.summary(ROUTE), TrafficSummary::default());
}
