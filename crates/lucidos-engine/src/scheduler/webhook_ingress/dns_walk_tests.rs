use std::collections::HashMap;

use axum::extract::Query;
use axum::routing::get;
use axum::Router;
use reqwest::StatusCode;

use super::*;

const TWO_A_RECORDS: &str = r#"{"Status":0,"Answer":[
    {"name":"node.tailnet.ts.net.","type":1,"TTL":300,"data":"203.0.113.7"},
    {"name":"node.tailnet.ts.net.","type":1,"TTL":300,"data":"198.51.100.4"}
]}"#;

const ONE_AAAA_RECORD: &str = r#"{"Status":0,"Answer":[
    {"name":"node.tailnet.ts.net.","type":28,"TTL":300,"data":"2001:db8::1"}
]}"#;

const OTHER_A_RECORD: &str = r#"{"Status":0,"Answer":[
    {"name":"node.tailnet.ts.net.","type":1,"TTL":300,"data":"203.0.113.99"}
]}"#;

const NXDOMAIN: &str = r#"{"Status":3,"Question":[{"name":"node.tailnet.ts.net","type":1}],
    "Authority":[{"name":"ts.net","type":6,"TTL":3368,"data":"ns1.example.com. admin.example.com. 1 2 3 4 5"}]}"#;

const NO_SUCH_TYPE: &str = r#"{"Status":0}"#;

const UNREADABLE: &str = "not json at all";

#[derive(Clone)]
struct Scripted {
    status: u16,
    a: &'static str,
    aaaa: &'static str,
}

impl Scripted {
    fn always(body: &'static str) -> Self {
        Self {
            status: 200,
            a: body,
            aaaa: body,
        }
    }

    fn status(status: u16) -> Self {
        Self {
            status,
            a: "",
            aaaa: "",
        }
    }

    fn per_type(a: &'static str, aaaa: &'static str) -> Self {
        Self {
            status: 200,
            a,
            aaaa,
        }
    }
}

async fn stub_resolvers(scripted: &[Scripted]) -> Vec<String> {
    let mut app = Router::new();
    for (index, answer) in scripted.iter().enumerate() {
        let answer = answer.clone();
        app = app.route(
            &format!("/{index}"),
            get(move |Query(params): Query<HashMap<String, String>>| {
                let answer = answer.clone();
                async move {
                    let wants_aaaa = params.get("type").map(String::as_str) == Some("28");
                    let body = if wants_aaaa { answer.aaaa } else { answer.a };
                    let status = StatusCode::from_u16(answer.status).unwrap_or(StatusCode::OK);
                    (status, body)
                }
            }),
        );
    }
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0")
        .await
        .expect("the stub binds a loopback port");
    let addr = listener.local_addr().expect("the stub has an address");
    tokio::spawn(async move {
        let _ = axum::serve(listener, app).await;
    });
    (0..scripted.len())
        .map(|index| format!("http://{addr}/{index}"))
        .collect()
}

fn client() -> reqwest::Client {
    reqwest::Client::builder()
        .no_proxy()
        .build()
        .expect("the test client builds")
}

async fn walk(scripted: &[Scripted], record_type: u16) -> Option<Vec<IpAddr>> {
    let urls = stub_resolvers(scripted).await;
    let endpoints: Vec<&str> = urls.iter().map(String::as_str).collect();
    resolve_one_type(&client(), &endpoints, "node.tailnet.ts.net", record_type).await
}

async fn resolve(scripted: &[Scripted]) -> PublicAddresses {
    let urls = stub_resolvers(scripted).await;
    let endpoints: Vec<&str> = urls.iter().map(String::as_str).collect();
    public_addresses_from(&client(), &endpoints, "node.tailnet.ts.net").await
}

fn ip(s: &str) -> IpAddr {
    s.parse().expect("test address parses")
}

#[tokio::test]
async fn a_refusal_from_one_resolver_does_not_settle_the_question() {
    for refusal in [NXDOMAIN, NO_SUCH_TYPE] {
        let found = walk(
            &[Scripted::always(refusal), Scripted::always(TWO_A_RECORDS)],
            1,
        )
        .await;
        assert_eq!(
            found,
            Some(vec![ip("203.0.113.7"), ip("198.51.100.4")]),
            "a second endpoint that names addresses has to be believed"
        );
    }
}

#[tokio::test]
async fn the_first_endpoint_that_names_an_address_wins() {
    let found = walk(
        &[
            Scripted::always(TWO_A_RECORDS),
            Scripted::always(OTHER_A_RECORD),
        ],
        1,
    )
    .await;
    assert_eq!(found, Some(vec![ip("203.0.113.7"), ip("198.51.100.4")]));
}

#[tokio::test]
async fn every_endpoint_naming_nothing_is_a_fact_about_the_host() {
    let found = walk(
        &[Scripted::always(NXDOMAIN), Scripted::always(NO_SUCH_TYPE)],
        1,
    )
    .await;
    assert_eq!(found, Some(Vec::new()));
}

#[tokio::test]
async fn no_endpoint_answering_is_not_an_empty_answer() {
    let found = walk(
        &[
            Scripted::status(500),
            Scripted::always(UNREADABLE),
            Scripted::status(429),
        ],
        1,
    )
    .await;
    assert_eq!(found, None);
}

#[tokio::test]
async fn a_lone_no_record_answer_beside_a_silent_endpoint_settles_nothing() {
    for scripted in [
        [Scripted::status(500), Scripted::always(NXDOMAIN)],
        [Scripted::always(NXDOMAIN), Scripted::always(UNREADABLE)],
    ] {
        assert_eq!(walk(&scripted, 1).await, None);
    }
}

#[tokio::test]
async fn a_host_with_no_aaaa_record_still_resolves_over_ipv4() {
    let answer = resolve(&[Scripted::per_type(TWO_A_RECORDS, NO_SUCH_TYPE)]).await;
    assert_eq!(
        answer,
        PublicAddresses::Found(vec![ip("198.51.100.4"), ip("203.0.113.7")])
    );
}

#[tokio::test]
async fn both_families_are_asked_and_both_answers_are_kept() {
    let answer = resolve(&[Scripted::per_type(TWO_A_RECORDS, ONE_AAAA_RECORD)]).await;
    assert_eq!(
        answer,
        PublicAddresses::Found(vec![
            ip("198.51.100.4"),
            ip("203.0.113.7"),
            ip("2001:db8::1"),
        ])
    );
}

#[tokio::test]
async fn a_hostname_nobody_has_a_record_for_reads_as_no_record() {
    let answer = resolve(&[Scripted::always(NXDOMAIN)]).await;
    assert_eq!(answer, PublicAddresses::NoRecord);
}

#[tokio::test]
async fn one_unanswered_family_does_not_discard_the_other_one() {
    let answer = resolve(&[Scripted::per_type(TWO_A_RECORDS, UNREADABLE)]).await;
    assert_eq!(
        answer,
        PublicAddresses::Found(vec![ip("198.51.100.4"), ip("203.0.113.7")])
    );
}

#[tokio::test]
async fn a_lookup_nobody_answered_reads_as_unknown() {
    for scripted in [
        Scripted::always(UNREADABLE),
        Scripted::per_type(UNREADABLE, NO_SUCH_TYPE),
    ] {
        assert_eq!(resolve(&[scripted]).await, PublicAddresses::Unknown);
    }
}
