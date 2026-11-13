use super::*;
use std::net::Ipv4Addr;

fn lan(last: u8) -> PairingChannel {
    PairingChannel::Local {
        source: IpAddr::V4(Ipv4Addr::new(192, 168, 1, last)),
    }
}

fn source(n: u32) -> PairingChannel {
    PairingChannel::Local {
        source: IpAddr::V4(Ipv4Addr::from(0x0a00_0000 + n)),
    }
}

fn device() -> PendingPairings {
    PendingPairings::new(PairingKind::Device)
}

fn gateway() -> PendingPairings {
    PendingPairings::new(PairingKind::Gateway)
}

/// A wrong long secret of `kind` that still has the minted shape, so the test
/// exercises the compare rather than the grammar.
fn wrong_secret(kind: PairingKind, n: u32) -> String {
    let body: String = format!("{n:026}")
        .bytes()
        .map(|b| char::from(BASE32[usize::from(b - b'0')]))
        .collect();
    format!("{}{body}", kind.secret_prefix())
}

fn short(minted: &MintedPairing) -> &str {
    minted
        .code
        .as_deref()
        .expect("a device pairing has a short code")
}

// ── Shapes ──────────────────────────────────────────────────────────────────

#[test]
fn a_minted_secret_is_its_prefix_and_26_base32_characters() {
    for kind in [PairingKind::Device, PairingKind::Gateway] {
        let secret = mint_secret(kind).unwrap();
        assert_eq!(valid_pairing_secret(&secret, kind), Some(secret.as_str()));
        assert_eq!(
            secret.len(),
            kind.secret_prefix().len() + PAIRING_SECRET_CHARS
        );
        assert_ne!(mint_secret(kind).unwrap(), secret, "secrets are random");
    }
}

#[test]
fn a_secret_of_one_kind_is_not_a_secret_of_the_other() {
    let device = mint_secret(PairingKind::Device).unwrap();
    assert_eq!(valid_pairing_secret(&device, PairingKind::Gateway), None);
    let gateway = mint_secret(PairingKind::Gateway).unwrap();
    assert_eq!(valid_pairing_secret(&gateway, PairingKind::Device), None);
}

#[test]
fn the_readers_refuse_anything_that_could_escape_a_json_string() {
    // A device token is echoed into the manifest's `start_url`, so the grammar
    // is the whole defence. Each of these is a caller's to send.
    let long = wrong_secret(PairingKind::Device, 1);
    for raw in [
        "",
        "   ",
        "0123456",
        "012345678",
        "0123456a",
        "0123 567",
        "0123456\"",
        "01234567\"}",
        "0123456\n",
        "../../etc",
        "01234567&x=1",
        "%30%31%32%33%34%35%36%37",
        &long[..long.len() - 1],
        &format!("{long}a"),
        &long.to_uppercase(),
        &format!("{}\"", &long[..long.len() - 1]),
        &long.replace("ldp_", "ldp-"),
    ] {
        assert_eq!(valid_device_pairing_token(raw), None, "accepted {raw:?}");
    }
    assert_eq!(valid_device_pairing_token("  01234567 "), Some("01234567"));
    assert_eq!(valid_device_pairing_token(&long), Some(long.as_str()));
}

// ── Redemption ──────────────────────────────────────────────────────────────

#[test]
fn a_long_secret_redeems_once_on_any_channel() {
    for channel in [PairingChannel::Public, lan(2)] {
        let mut pending = device();
        let minted = pending.mint(PairingIntent::labelled("My iPhone")).unwrap();
        assert_eq!(
            pending.redeem(&minted.secret, channel, OwnerGate::NoDevicePaired),
            Redemption::Redeemed(PairingIntent::labelled("My iPhone"))
        );
        assert_eq!(
            pending.redeem(&minted.secret, channel, OwnerGate::NoDevicePaired),
            Redemption::Rejected
        );
    }
}

#[test]
fn redeeming_either_half_spends_the_whole_pairing() {
    let mut pending = device();
    let minted = pending.mint(PairingIntent::default()).unwrap();
    assert_eq!(
        pending.redeem(short(&minted), lan(2), OwnerGate::NoDevicePaired),
        Redemption::Redeemed(PairingIntent::default())
    );
    assert_eq!(
        pending.redeem(&minted.secret, lan(2), OwnerGate::NoDevicePaired),
        Redemption::Rejected,
        "the secret died with its short code"
    );
}

#[test]
fn a_short_code_from_a_public_channel_is_refused_before_any_compare() {
    let mut pending = device();
    let minted = pending.mint(PairingIntent::default()).unwrap();
    assert_eq!(
        pending.redeem(
            short(&minted),
            PairingChannel::Public,
            OwnerGate::NoDevicePaired
        ),
        Redemption::LocalOnly
    );
    assert_eq!(
        pending.redeem(
            &minted.secret,
            PairingChannel::Public,
            OwnerGate::NoDevicePaired
        ),
        Redemption::Redeemed(PairingIntent::default()),
        "the refusal spent nothing"
    );
}

#[test]
fn a_gateway_pairing_has_no_short_code_on_any_channel() {
    let mut pending = gateway();
    let minted = pending.mint(PairingIntent::default()).unwrap();
    assert_eq!(minted.code, None);
    assert!(minted.secret.starts_with("lgp_"));
    assert_eq!(
        pending.redeem("01234567", lan(2), OwnerGate::NoDevicePaired),
        Redemption::Rejected
    );
    assert_eq!(
        pending.redeem(
            &minted.secret,
            PairingChannel::Public,
            OwnerGate::NoDevicePaired
        ),
        Redemption::Redeemed(PairingIntent::default())
    );
}

#[test]
fn a_pairing_expires() {
    let mut pending = device();
    let minted = pending.mint(PairingIntent::default()).unwrap();
    let later = Instant::now() + PAIRING_TTL;
    assert_eq!(
        pending.redeem_at(
            &minted.secret,
            PairingChannel::Public,
            OwnerGate::NoDevicePaired,
            later
        ),
        Redemption::Rejected
    );
}

// ── Floods ──────────────────────────────────────────────────────────────────

/// Invariant: an unlimited distributed flood of wrong secrets cannot stop a
/// correct long secret from pairing, for device and gateway pairing alike.
#[test]
fn a_distributed_flood_never_stops_a_correct_long_secret() {
    for kind in [PairingKind::Device, PairingKind::Gateway] {
        let mut pending = PendingPairings::new(kind);
        let over_public = pending.mint(PairingIntent::default()).unwrap();
        let over_lan = pending.mint(PairingIntent::default()).unwrap();
        let minted_codes = [over_public.code.clone(), over_lan.code.clone()];
        for n in 0..100_000u32 {
            let channel = if n % 2 == 0 {
                source(n % 1_000)
            } else {
                PairingChannel::Public
            };
            let guess = if n % 3 == 0 {
                format!("{:08}", n % 100_000_000)
            } else {
                wrong_secret(kind, n)
            };
            if minted_codes.contains(&Some(guess.clone())) {
                continue;
            }
            assert_ne!(
                pending.redeem(&guess, channel, OwnerGate::NoDevicePaired),
                Redemption::Redeemed(PairingIntent::default()),
                "a wrong guess paired"
            );
        }
        for (minted, channel) in [(over_public, PairingChannel::Public), (over_lan, source(2))] {
            assert_eq!(
                pending.redeem(&minted.secret, channel, OwnerGate::NoDevicePaired),
                Redemption::Redeemed(PairingIntent::default()),
                "{kind:?}: the flood refused the correct secret on {channel:?}"
            );
        }
    }
}

/// Invariant: with no pairing pending, a redemption counts nothing.
#[test]
fn a_flood_with_nothing_pending_costs_the_next_pairing_nothing() {
    let mut pending = device();
    for _ in 0..1_000 {
        assert_eq!(
            pending.redeem("00000000", lan(9), OwnerGate::NoDevicePaired),
            Redemption::Rejected
        );
    }
    let minted = pending.mint(PairingIntent::default()).unwrap();
    assert_eq!(
        pending.redeem(short(&minted), lan(9), OwnerGate::NoDevicePaired),
        Redemption::Redeemed(PairingIntent::default())
    );
}

/// Invariant: a junk flood from X cannot block a correct short code from Y.
#[test]
fn one_local_source_spends_only_its_own_budget() {
    let mut pending = device();
    let minted = pending.mint(PairingIntent::default()).unwrap();
    let wrong = if short(&minted) == "00000000" {
        "00000001"
    } else {
        "00000000"
    };
    for _ in 0..SHORT_CODE_FAILURES_PER_SOURCE {
        assert_eq!(
            pending.redeem(wrong, lan(66), OwnerGate::NoDevicePaired),
            Redemption::Rejected
        );
    }
    for _ in 0..1_000 {
        assert_eq!(
            pending.redeem(wrong, lan(66), OwnerGate::NoDevicePaired),
            Redemption::Throttled
        );
    }
    assert_eq!(
        pending.redeem(short(&minted), lan(66), OwnerGate::NoDevicePaired),
        Redemption::Throttled,
        "a spent source is not even compared"
    );
    assert_eq!(
        pending.redeem(short(&minted), lan(7), OwnerGate::NoDevicePaired),
        Redemption::Redeemed(PairingIntent::default())
    );
}

#[test]
fn an_ipv6_host_cannot_rotate_its_way_to_a_fresh_budget() {
    let mut pending = device();
    let minted = pending.mint(PairingIntent::default()).unwrap();
    let wrong = if short(&minted) == "00000000" {
        "00000001"
    } else {
        "00000000"
    };
    let in_one_64 = |n: u16| {
        channel(
            peer(&format!("[fd00:0:0:1::{n:x}]:4000"), "[fd00::5]:5252"),
            &[],
        )
    };
    for n in 0..SHORT_CODE_FAILURES_PER_SOURCE as u16 {
        assert_eq!(
            pending.redeem(wrong, in_one_64(n), OwnerGate::NoDevicePaired),
            Redemption::Rejected
        );
    }
    assert_eq!(
        pending.redeem(short(&minted), in_one_64(999), OwnerGate::NoDevicePaired),
        Redemption::Throttled
    );
}

/// Every tailnet node shares one /64, and the control server assigns each its
/// address. So a tailnet peer keeps a budget of its own.
#[test]
fn one_tailnet_device_cannot_spend_another_ones_budget() {
    let mut pending = device();
    let minted = pending.mint(PairingIntent::default()).unwrap();
    let wrong = if short(&minted) == "00000000" {
        "00000001"
    } else {
        "00000000"
    };
    let node = |n: u16| {
        let remote = format!("[fd7a:115c:a1e0::{n:x}]:4000");
        classify(
            peer(&remote, "[fd7a:115c:a1e0::1]:5252"),
            &HeaderMap::new(),
            |ip| ip == "fd7a:115c:a1e0::1".parse::<IpAddr>().unwrap(),
        )
    };
    for _ in 0..SHORT_CODE_FAILURES_PER_SOURCE {
        assert_eq!(
            pending.redeem(wrong, node(2), OwnerGate::NoDevicePaired),
            Redemption::Rejected
        );
    }
    assert_eq!(
        pending.redeem(short(&minted), node(2), OwnerGate::NoDevicePaired),
        Redemption::Throttled
    );
    assert_eq!(
        pending.redeem(short(&minted), node(3), OwnerGate::NoDevicePaired),
        Redemption::Redeemed(PairingIntent::default())
    );
}

#[test]
fn the_backstop_caps_many_local_sources_and_never_a_long_secret() {
    let mut pending = device();
    let minted = pending.mint(PairingIntent::default()).unwrap();
    let wrong = if short(&minted) == "00000000" {
        "00000001"
    } else {
        "00000000"
    };
    let sources = SHORT_CODE_FAILURES_GLOBAL / SHORT_CODE_FAILURES_PER_SOURCE;
    for n in 0..sources {
        for _ in 0..SHORT_CODE_FAILURES_PER_SOURCE {
            assert_eq!(
                pending.redeem(wrong, source(n), OwnerGate::NoDevicePaired),
                Redemption::Rejected
            );
        }
    }
    assert_eq!(
        pending.redeem(short(&minted), source(500), OwnerGate::NoDevicePaired),
        Redemption::Throttled
    );
    assert_eq!(
        pending.redeem(&minted.secret, source(500), OwnerGate::NoDevicePaired),
        Redemption::Redeemed(PairingIntent::default()),
        "the long secret is never throttled"
    );
}

#[test]
fn a_correct_short_code_clears_its_source_and_the_window_refills() {
    let mut pending = device();
    let first = pending.mint(PairingIntent::default()).unwrap();
    let wrong = |m: &MintedPairing| {
        if short(m) == "00000000" {
            "00000001"
        } else {
            "00000000"
        }
    };
    for _ in 0..SHORT_CODE_FAILURES_PER_SOURCE - 1 {
        assert_eq!(
            pending.redeem(wrong(&first), lan(3), OwnerGate::NoDevicePaired),
            Redemption::Rejected
        );
    }
    assert_eq!(
        pending.redeem(short(&first), lan(3), OwnerGate::NoDevicePaired),
        Redemption::Redeemed(PairingIntent::default())
    );
    let second = pending.mint(PairingIntent::default()).unwrap();
    for _ in 0..SHORT_CODE_FAILURES_PER_SOURCE {
        assert_eq!(
            pending.redeem(wrong(&second), lan(3), OwnerGate::NoDevicePaired),
            Redemption::Rejected
        );
    }
    let later = Instant::now() + FAILURE_WINDOW;
    assert_eq!(
        pending.redeem_at(short(&second), lan(3), OwnerGate::NoDevicePaired, later),
        Redemption::Redeemed(PairingIntent::default())
    );
}

// ── Owner approval ──────────────────────────────────────────────────────────

fn minted_by(minter: Minter) -> PairingIntent {
    PairingIntent {
        minter,
        ..PairingIntent::default()
    }
}

const OWNER: OwnerGate<'static> = OwnerGate::DevicePaired {
    asking_as: "My iPhone",
};

/// The one request waiting, answered by id the way a paired device would.
fn only_request(pending: &mut PendingPairings) -> PairingRequest {
    let requests = pending.requests();
    assert_eq!(requests.len(), 1, "{requests:?}");
    requests.into_iter().next().unwrap()
}

#[test]
fn a_pairing_counts_as_a_local_process_s_unless_a_device_minted_it() {
    assert_eq!(PairingIntent::default().minter, Minter::LocalProcess);
}

/// Invariant 9: with no device paired, a local process's code redeems at once.
#[test]
fn a_local_process_s_code_redeems_at_once_with_no_device_paired() {
    for kind in [PairingKind::Device, PairingKind::Gateway] {
        let mut pending = PendingPairings::new(kind);
        let minted = pending.mint(minted_by(Minter::LocalProcess)).unwrap();
        assert_eq!(
            pending.redeem(
                &minted.secret,
                PairingChannel::Public,
                OwnerGate::NoDevicePaired
            ),
            Redemption::Redeemed(minted_by(Minter::LocalProcess))
        );
    }
}

/// Invariant 9: a paired device's own code never waits.
#[test]
fn a_paired_device_s_code_redeems_at_once() {
    for kind in [PairingKind::Device, PairingKind::Gateway] {
        let mut pending = PendingPairings::new(kind);
        let minted = pending.mint(minted_by(Minter::PairedDevice)).unwrap();
        assert_eq!(
            pending.redeem(&minted.secret, PairingChannel::Public, OWNER),
            Redemption::Redeemed(minted_by(Minter::PairedDevice))
        );
        assert!(pending.requests().is_empty());
    }
}

/// Invariant 9: with a device paired, a local process's code waits for Allow.
#[test]
fn a_local_process_s_code_waits_for_allow_then_redeems() {
    for kind in [PairingKind::Device, PairingKind::Gateway] {
        let mut pending = PendingPairings::new(kind);
        let minted = pending.mint(minted_by(Minter::LocalProcess)).unwrap();
        // Only the first redeem starts the wait, so the owner hears it once.
        for started in [true, false, false] {
            assert_eq!(
                pending.redeem(&minted.secret, PairingChannel::Public, OWNER),
                Redemption::AwaitingOwner { started },
                "{kind:?}: asking again never pairs"
            );
        }
        let request = only_request(&mut pending);
        assert_eq!(request.label, "My iPhone");
        assert!(pending.answer(&request.id, PairingAnswer::Allow));
        assert_eq!(
            pending.redeem(&minted.secret, PairingChannel::Public, OWNER),
            Redemption::Redeemed(minted_by(Minter::LocalProcess))
        );
        assert_eq!(
            pending.redeem(&minted.secret, PairingChannel::Public, OWNER),
            Redemption::Rejected,
            "an allowed pairing still redeems once"
        );
    }
}

#[test]
fn don_t_allow_is_heard_once_and_then_the_pairing_is_gone() {
    let mut pending = device();
    let minted = pending.mint(minted_by(Minter::LocalProcess)).unwrap();
    assert_eq!(
        pending.redeem(&minted.secret, PairingChannel::Public, OWNER),
        Redemption::AwaitingOwner { started: true }
    );
    let request = only_request(&mut pending);
    assert!(pending.answer(&request.id, PairingAnswer::Decline));
    assert!(
        pending.requests().is_empty(),
        "a declined request is no longer asked"
    );
    assert_eq!(
        pending.redeem(&minted.secret, PairingChannel::Public, OWNER),
        Redemption::Declined
    );
    assert_eq!(
        pending.redeem(&minted.secret, PairingChannel::Public, OWNER),
        Redemption::Rejected
    );
}

#[test]
fn a_short_code_waits_the_same_way() {
    let mut pending = device();
    let minted = pending.mint(minted_by(Minter::LocalProcess)).unwrap();
    assert_eq!(
        pending.redeem(short(&minted), lan(4), OWNER),
        Redemption::AwaitingOwner { started: true }
    );
    let request = only_request(&mut pending);
    assert!(pending.answer(&request.id, PairingAnswer::Allow));
    assert_eq!(
        pending.redeem(short(&minted), lan(4), OWNER),
        Redemption::Redeemed(minted_by(Minter::LocalProcess))
    );
}

#[test]
fn nothing_is_asked_until_someone_redeems() {
    let mut pending = device();
    pending.mint(minted_by(Minter::LocalProcess)).unwrap();
    assert!(pending.requests().is_empty());
}

#[test]
fn the_window_restarts_when_the_redeem_starts_waiting() {
    let mut pending = device();
    let minted = pending.mint(minted_by(Minter::LocalProcess)).unwrap();
    let claimed = Instant::now() + PAIRING_TTL - Duration::from_secs(1);
    assert_eq!(
        pending.redeem_at(&minted.secret, PairingChannel::Public, OWNER, claimed),
        Redemption::AwaitingOwner { started: true }
    );
    let request = pending.requests_at(claimed).into_iter().next().unwrap();
    assert_eq!(request.expires_in_secs, PAIRING_TTL.as_secs());
    assert!(pending.answer(&request.id, PairingAnswer::Allow));
    let still_open = claimed + PAIRING_TTL - Duration::from_secs(1);
    assert_eq!(
        pending.redeem_at(&minted.secret, PairingChannel::Public, OWNER, still_open),
        Redemption::Redeemed(minted_by(Minter::LocalProcess))
    );
}

#[test]
fn a_waiting_request_expires_with_its_window() {
    let mut pending = device();
    let minted = pending.mint(minted_by(Minter::LocalProcess)).unwrap();
    assert_eq!(
        pending.redeem(&minted.secret, PairingChannel::Public, OWNER),
        Redemption::AwaitingOwner { started: true }
    );
    let later = Instant::now() + PAIRING_TTL;
    assert!(pending.requests_at(later).is_empty());
    let id = only_request(&mut pending).id;
    pending.drop_expired(later);
    assert!(
        !pending.answer(&id, PairingAnswer::Allow),
        "nothing left to allow"
    );
}

#[test]
fn an_unknown_request_id_answers_nothing() {
    let mut pending = device();
    assert!(!pending.answer("no-such-request", PairingAnswer::Allow));
}

/// Revoking every device reopens the free pass, as the owner settled
/// (plan § Phase 6 Design). A request already waiting then redeems.
#[test]
fn a_waiting_code_redeems_once_no_device_is_paired() {
    let mut pending = device();
    let minted = pending.mint(minted_by(Minter::LocalProcess)).unwrap();
    assert_eq!(
        pending.redeem(&minted.secret, PairingChannel::Public, OWNER),
        Redemption::AwaitingOwner { started: true }
    );
    assert_eq!(
        pending.redeem(
            &minted.secret,
            PairingChannel::Public,
            OwnerGate::NoDevicePaired
        ),
        Redemption::Redeemed(minted_by(Minter::LocalProcess))
    );
}

// ── Channels ────────────────────────────────────────────────────────────────

const TAILNET_SELF: IpAddr = IpAddr::V4(Ipv4Addr::new(100, 100, 1, 1));

fn peer(remote: &str, local: &str) -> Option<Peer> {
    Some(Peer {
        remote: remote.parse().unwrap(),
        local: local.parse().unwrap(),
        public_listener: false,
    })
}

fn headers(pairs: &[(&str, &str)]) -> HeaderMap {
    let mut h = HeaderMap::new();
    for (k, v) in pairs {
        h.insert(
            axum::http::HeaderName::from_bytes(k.as_bytes()).unwrap(),
            v.parse().unwrap(),
        );
    }
    h
}

fn channel(peer: Option<Peer>, pairs: &[(&str, &str)]) -> PairingChannel {
    classify(peer, &headers(pairs), |ip| ip == TAILNET_SELF)
}

/// Invariant: a loopback peer is refused a short code, whatever its headers.
/// cloudflared, `tailscale serve`, `ssh -L` and a raw TCP forward all connect
/// from loopback, and a raw forward passes any `Host` its caller chose.
#[test]
fn every_loopback_request_is_public() {
    for (remote, local) in [
        ("127.0.0.1:50000", "127.0.0.1:5252"),
        ("[::1]:50000", "[::1]:5252"),
        ("[::ffff:127.0.0.1]:50000", "[::ffff:127.0.0.1]:5252"),
    ] {
        let loopback = peer(remote, local);
        assert_eq!(channel(loopback, &[]), PairingChannel::Public, "{remote}");
        for name in FORWARDING_HEADERS {
            assert_eq!(
                channel(loopback, &[(name, "203.0.113.9")]),
                PairingChannel::Public,
                "{remote} {name}"
            );
        }
    }
}

/// Invariant: a forwarder on this machine aimed at its own LAN or tailnet
/// address connects from that address, so it is public too.
#[test]
fn a_caller_on_this_machine_is_public_on_every_address() {
    for addr in ["192.168.1.5", "100.100.1.1", "[fd12::5]"] {
        let to_itself = peer(&format!("{addr}:50000"), &format!("{addr}:5252"));
        assert_eq!(channel(to_itself, &[]), PairingChannel::Public, "{addr}");
    }
}

#[test]
fn every_request_on_the_public_listener_is_public() {
    let mut on_public = peer("192.168.1.20:50000", "192.168.1.5:5272");
    on_public.as_mut().unwrap().public_listener = true;
    assert_eq!(channel(on_public, &[]), PairingChannel::Public);
}

#[test]
fn a_direct_lan_peer_is_local_and_a_public_peer_is_not() {
    for remote in [
        "192.168.1.20:4000",
        "10.1.2.3:4000",
        "172.16.0.9:4000",
        "[fd12::5]:4000",
    ] {
        assert!(
            matches!(
                channel(peer(remote, "0.0.0.0:5252"), &[]),
                PairingChannel::Local { .. }
            ),
            "{remote}"
        );
    }
    for remote in ["203.0.113.9:4000", "[2001:db8::1]:4000", "100.70.0.2:4000"] {
        assert_eq!(
            channel(peer(remote, "192.168.1.5:5252"), &[]),
            PairingChannel::Public,
            "{remote}"
        );
    }
}

#[test]
fn a_peer_arriving_on_a_tailnet_address_of_this_machine_is_local() {
    let on_tailnet = peer("100.70.0.2:4000", "100.100.1.1:5252");
    assert!(matches!(
        channel(on_tailnet, &[]),
        PairingChannel::Local { .. }
    ));
}

/// Invariant: a spoofed forwarding header from an untrusted hop earns no
/// budget and still marks the request public.
#[test]
fn a_spoofed_forwarding_header_neither_spends_nor_earns_a_budget() {
    let mut pending = device();
    let minted = pending.mint(PairingIntent::default()).unwrap();
    let lan_peer = peer("192.168.1.40:4000", "192.168.1.5:5252");
    for n in 0..1_000 {
        let spoof = format!("10.9.{}.{}", n / 256, n % 256);
        let ch = channel(lan_peer, &[("x-forwarded-for", &spoof)]);
        assert_eq!(
            pending.redeem("00000000", ch, OwnerGate::NoDevicePaired),
            Redemption::LocalOnly
        );
    }
    let direct = channel(lan_peer, &[]);
    assert_eq!(
        pending.redeem(short(&minted), direct, OwnerGate::NoDevicePaired),
        Redemption::Redeemed(PairingIntent::default())
    );
}

#[test]
fn no_stamped_peer_is_public() {
    assert_eq!(channel(None, &[]), PairingChannel::Public);
}

#[test]
fn the_public_listener_derives_from_the_gateway_port() {
    assert_eq!(public_port(None, 5251), Some(5281));
    assert_eq!(public_port(None, 5252), Some(5282));
    assert_eq!(public_port(Some("9100"), 5252), Some(9100));
    assert_eq!(public_port(Some("0"), 5252), None);
    assert_eq!(public_port(Some("nonsense"), 5252), None);
}
