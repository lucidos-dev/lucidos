use super::*;

fn probe(address: &str, family: Family, stage: Stage) -> AddressProbe {
    AddressProbe {
        address: address.to_string(),
        family,
        stage,
        status: match stage {
            Stage::Healthy => Some(401),
            Stage::BackendUnreachable => Some(502),
            Stage::RouteMissing => Some(404),
            _ => None,
        },
        detail: None,
    }
}

fn verdict_of(families: &[FamilyVerdict], family: Family) -> FamilyVerdict {
    *families
        .iter()
        .find(|f| f.family == family)
        .expect("both families are always reported")
}

fn reading(ipv4: Verdict, ipv6: Verdict) -> Vec<FamilyVerdict> {
    Family::BOTH
        .iter()
        .zip([ipv4, ipv6])
        .map(|(family, verdict)| FamilyVerdict {
            family: *family,
            verdict,
            healthy: usize::from(verdict == Verdict::Healthy),
            total: usize::from(verdict != Verdict::NotProbed),
        })
        .collect()
}

#[test]
fn a_dead_family_is_degraded_even_when_the_other_one_is_perfect() {
    let addresses = vec![
        probe("203.0.113.7", Family::Ipv4, Stage::IngressUnreachable),
        probe("203.0.113.8", Family::Ipv4, Stage::IngressUnreachable),
        probe("203.0.113.9", Family::Ipv4, Stage::IngressUnreachable),
        probe("2001:db8::1", Family::Ipv6, Stage::Healthy),
    ];
    let families = judge(&addresses);

    assert_eq!(
        verdict_of(&families, Family::Ipv4).verdict,
        Verdict::Degraded
    );
    assert_eq!(verdict_of(&families, Family::Ipv4).healthy, 0);
    assert_eq!(verdict_of(&families, Family::Ipv4).total, 3);
    assert_eq!(
        verdict_of(&families, Family::Ipv6).verdict,
        Verdict::Healthy
    );
    assert_eq!(degraded_families(&families), vec![Family::Ipv4]);
}

#[test]
fn one_healthy_address_carries_its_family() {
    let addresses = vec![
        probe("203.0.113.7", Family::Ipv4, Stage::IngressUnreachable),
        probe("203.0.113.8", Family::Ipv4, Stage::Healthy),
    ];
    let families = judge(&addresses);

    assert_eq!(
        verdict_of(&families, Family::Ipv4).verdict,
        Verdict::Healthy
    );
    assert_eq!(verdict_of(&families, Family::Ipv4).healthy, 1);
    assert_eq!(verdict_of(&families, Family::Ipv4).total, 2);
    assert!(degraded_families(&families).is_empty());
}

#[test]
fn a_family_this_host_cannot_reach_is_not_probed_rather_than_degraded() {
    let addresses = vec![
        probe("203.0.113.7", Family::Ipv4, Stage::Healthy),
        probe("2001:db8::1", Family::Ipv6, Stage::LocalStackUnavailable),
    ];
    let families = judge(&addresses);

    let v6 = verdict_of(&families, Family::Ipv6);
    assert_eq!(v6.verdict, Verdict::NotProbed);
    assert_eq!(v6.total, 0);
    assert!(degraded_families(&families).is_empty());
}

#[test]
fn a_port_this_host_cannot_open_is_not_probed_rather_than_degraded() {
    let addresses = vec![
        probe("203.0.113.7", Family::Ipv4, Stage::LocalEgressBlocked),
        probe("203.0.113.8", Family::Ipv4, Stage::LocalEgressBlocked),
        probe("2001:db8::1", Family::Ipv6, Stage::Healthy),
    ];
    let families = judge(&addresses);

    let v4 = verdict_of(&families, Family::Ipv4);
    assert_eq!(v4.verdict, Verdict::NotProbed);
    assert_eq!(v4.total, 0);
    assert!(degraded_families(&families).is_empty());
}

#[test]
fn one_family_blocked_locally_does_not_hide_the_other_one_failing() {
    let addresses = vec![
        probe("203.0.113.7", Family::Ipv4, Stage::LocalEgressBlocked),
        probe("2001:db8::1", Family::Ipv6, Stage::IngressUnreachable),
    ];
    let families = judge(&addresses);

    assert_eq!(degraded_families(&families), vec![Family::Ipv6]);
    assert_eq!(
        verdict_of(&families, Family::Ipv4).verdict,
        Verdict::NotProbed
    );
}

#[test]
fn only_the_two_local_stages_measure_nothing() {
    assert!(!Stage::LocalStackUnavailable.measured_the_ingress());
    assert!(!Stage::LocalEgressBlocked.measured_the_ingress());
    for stage in [
        Stage::Healthy,
        Stage::IngressUnreachable,
        Stage::BackendUnreachable,
        Stage::RouteMissing,
        Stage::UnexpectedResponder,
    ] {
        assert!(stage.measured_the_ingress(), "{stage:?}");
    }
}

#[test]
fn a_family_that_answered_anywhere_is_never_blamed_on_this_host() {
    let mixed = vec![
        probe("203.0.113.7", Family::Ipv4, Stage::IngressUnreachable),
        probe("203.0.113.8", Family::Ipv4, Stage::BackendUnreachable),
    ];
    assert!(!nothing_answered(&mixed, Family::Ipv4));
    assert_eq!(degraded_families(&judge(&mixed)), vec![Family::Ipv4]);

    let all_dead = vec![
        probe("203.0.113.7", Family::Ipv4, Stage::IngressUnreachable),
        probe("203.0.113.8", Family::Ipv4, Stage::IngressUnreachable),
    ];
    assert!(nothing_answered(&all_dead, Family::Ipv4));

    assert!(!nothing_answered(&all_dead, Family::Ipv6));
    assert!(!nothing_answered(&[], Family::Ipv4));
    let no_route = vec![probe(
        "2001:db8::1",
        Family::Ipv6,
        Stage::LocalStackUnavailable,
    )];
    assert!(!nothing_answered(&no_route, Family::Ipv6));
}

#[test]
fn both_families_are_always_reported() {
    let families = judge(&[probe("203.0.113.7", Family::Ipv4, Stage::Healthy)]);
    assert_eq!(families.len(), 2);
    assert_eq!(families[0].family, Family::Ipv4);
    assert_eq!(families[1].family, Family::Ipv6);
    assert_eq!(
        verdict_of(&families, Family::Ipv6).verdict,
        Verdict::NotProbed
    );
}

#[test]
fn no_verdict_is_pronounced_over_zero_measurements() {
    let families = judge(&[]);
    assert!(degraded_families(&families).is_empty());
    for family in families {
        assert_eq!(family.verdict, Verdict::NotProbed);
        assert_eq!(family.total, 0);
        assert_eq!(family.healthy, 0);
    }
}

#[test]
fn the_status_code_is_the_diagnosis() {
    assert_eq!(classify_status(401), Stage::Healthy);
    assert_eq!(classify_status(502), Stage::BackendUnreachable);
    assert_eq!(classify_status(503), Stage::BackendUnreachable);
    assert_eq!(classify_status(504), Stage::BackendUnreachable);
    assert_eq!(classify_status(404), Stage::RouteMissing);
}

#[test]
fn a_success_means_something_that_is_not_lucidos_answered() {
    assert_eq!(classify_status(200), Stage::UnexpectedResponder);
    assert_eq!(classify_status(204), Stage::UnexpectedResponder);
    assert_eq!(classify_status(301), Stage::UnexpectedResponder);
    assert_eq!(classify_status(403), Stage::UnexpectedResponder);
    assert_eq!(classify_status(500), Stage::UnexpectedResponder);
}

#[test]
fn every_wire_name_is_kebab_case() {
    let json = serde_json::to_value(FamilyVerdict {
        family: Family::Ipv4,
        verdict: Verdict::NotProbed,
        healthy: 0,
        total: 0,
    })
    .unwrap();
    assert_eq!(json["family"], "ipv4");
    assert_eq!(json["verdict"], "not-probed");

    for (stage, name) in [
        (Stage::Healthy, "healthy"),
        (Stage::IngressUnreachable, "ingress-unreachable"),
        (Stage::BackendUnreachable, "backend-unreachable"),
        (Stage::RouteMissing, "route-missing"),
        (Stage::UnexpectedResponder, "unexpected-responder"),
        (Stage::LocalStackUnavailable, "local-stack-unavailable"),
        (Stage::LocalEgressBlocked, "local-egress-blocked"),
    ] {
        assert_eq!(serde_json::to_value(stage).unwrap(), name);
    }
    assert_eq!(serde_json::to_value(Family::Ipv6).unwrap(), "ipv6");
}

#[test]
fn a_null_status_and_detail_stay_in_the_payload() {
    let json = serde_json::to_value(probe(
        "203.0.113.7",
        Family::Ipv4,
        Stage::IngressUnreachable,
    ))
    .unwrap();
    assert!(json.get("status").is_some_and(serde_json::Value::is_null));
    assert!(json.get("detail").is_some_and(serde_json::Value::is_null));
}

#[test]
fn one_bad_cycle_declares_nothing() {
    let down = reading(Verdict::Degraded, Verdict::Healthy);
    let (decision, strikes) = decide(&down, &[], 0, None);
    assert_eq!(decision, Decision::Nothing);
    assert_eq!(strikes, 1);
}

#[test]
fn the_second_matching_cycle_declares() {
    let down = reading(Verdict::Degraded, Verdict::Healthy);
    let (_, strikes) = decide(&down, &[], 0, None);
    let (decision, strikes) = decide(&down, &[Family::Ipv4], strikes, None);
    assert_eq!(decision, Decision::Declare);
    assert_eq!(strikes, 2);
}

#[test]
fn a_declared_outage_is_not_declared_again() {
    let down = reading(Verdict::Degraded, Verdict::Healthy);
    let declared = [Family::Ipv4];
    let (decision, _) = decide(&down, &[Family::Ipv4], 9, Some(&declared));
    assert_eq!(decision, Decision::Nothing);
}

#[test]
fn a_second_family_falling_over_is_debounced_on_its_own_terms() {
    let declared = [Family::Ipv4];
    let both = reading(Verdict::Degraded, Verdict::Degraded);
    let (decision, strikes) = decide(&both, &[Family::Ipv4], 4, Some(&declared));
    assert_eq!(decision, Decision::Nothing);
    assert_eq!(strikes, 1);

    let seen = vec![Family::Ipv4, Family::Ipv6];
    let (decision, _) = decide(&both, &seen, strikes, Some(&declared));
    assert_eq!(decision, Decision::Declare);
}

#[test]
fn recovery_takes_a_single_clean_cycle() {
    let declared = [Family::Ipv4];
    let up = reading(Verdict::Healthy, Verdict::Healthy);
    let (decision, _) = decide(&up, &[Family::Ipv4], 7, Some(&declared));
    assert_eq!(decision, Decision::Recover);
}

#[test]
fn a_family_nobody_could_probe_never_retracts_its_own_outage() {
    let declared = [Family::Ipv6];
    let unprobed = reading(Verdict::Healthy, Verdict::NotProbed);
    assert!(degraded_families(&unprobed).is_empty());

    let (decision, _) = decide(&unprobed, &[], 7, Some(&declared));
    assert_eq!(decision, Decision::Nothing);

    let up = reading(Verdict::Healthy, Verdict::Healthy);
    let (decision, _) = decide(&up, &[], 7, Some(&declared));
    assert_eq!(decision, Decision::Recover);
}

#[test]
fn a_healthy_ingress_that_was_never_degraded_stays_quiet() {
    let up = reading(Verdict::Healthy, Verdict::Healthy);
    let (decision, _) = decide(&up, &[], 0, None);
    assert_eq!(decision, Decision::Nothing);
}

#[test]
fn the_declared_set_is_compared_in_canonical_order() {
    let addresses = vec![
        probe("2001:db8::1", Family::Ipv6, Stage::IngressUnreachable),
        probe("203.0.113.7", Family::Ipv4, Stage::IngressUnreachable),
    ];
    let families = judge(&addresses);
    let observed = degraded_families(&families);
    assert_eq!(observed, vec![Family::Ipv4, Family::Ipv6]);

    let (decision, _) = decide(&families, &observed, 5, Some(&observed));
    assert_eq!(decision, Decision::Nothing);
}
