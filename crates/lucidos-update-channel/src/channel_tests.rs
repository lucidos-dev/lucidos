use super::*;

#[test]
fn channels_round_trip_without_losing_custom_names() {
    for raw in [
        "stable",
        "beta",
        "nightly",
        "local",
        "custom:Custom address",
        "custom:Prøve",
    ] {
        assert_eq!(Channel::parse(raw).unwrap().as_str(), raw);
    }
    for raw in [
        "",
        "Stable",
        "unknown",
        "custom:",
        "custom: name",
        "custom:name ",
        "custom:a\nb",
        "custom:a/b",
        "custom:a\\b",
        "custom:a:b",
    ] {
        assert!(Channel::parse(raw).is_err(), "{raw:?}");
    }
    assert!(Channel::parse(&format!("custom:{}", "a".repeat(65))).is_err());
}

#[test]
fn channel_availability_covers_every_install_shape() {
    for shape in [
        None,
        Some(InstallShape::InstallerRerun),
        Some(InstallShape::DesktopApp),
    ] {
        for name in ["stable", "custom:Test"] {
            assert!(Channel::parse(name).unwrap().serves(shape));
        }
        assert!(!Channel::Nightly.serves(shape));
        for channel in [Channel::Beta, Channel::Local] {
            assert_eq!(
                channel.serves(shape),
                shape == Some(InstallShape::DesktopApp)
            );
        }
    }
}

#[test]
fn only_published_channels_have_a_stability_order() {
    assert!(Channel::Stable.stability_rank() < Channel::Beta.stability_rank());
    assert!(Channel::Beta.stability_rank() < Channel::Nightly.stability_rank());
    assert_eq!(Channel::Local.stability_rank(), None);
    assert_eq!(
        Channel::parse("custom:Test").unwrap().stability_rank(),
        None
    );
}

#[test]
fn channel_feeds_keep_the_transport_rule() {
    for url in [
        BETA_FEED_URL,
        NIGHTLY_FEED_URL,
        "http://localhost:12345/~/updates/local/latest.json",
    ] {
        assert!(UpdateAddress::parse(url).unwrap().is_some());
    }
}

#[test]
fn every_ordered_version_pair_obeys_semver_precedence() {
    for (i, current) in ORDERED_VERSIONS.iter().enumerate() {
        for (j, offered) in ORDERED_VERSIONS.iter().enumerate() {
            assert_eq!(
                is_upgrade(current, offered),
                j > i,
                "{current} -> {offered}"
            );
        }
    }
}

#[test]
fn metadata_is_not_an_upgrade_and_invalid_versions_fail_closed() {
    assert!(!is_upgrade("1.2.3+one", "1.2.3+two"));
    assert!(!is_upgrade("1.2.3", "1.2.3+two"));
    assert!(is_upgrade(" 1.2.3 ", " 1.2.4 "));
    for invalid in [
        "",
        "v1.2.3",
        "1.2",
        "1.2.3.4",
        "1.02.3",
        "1.2.3-nightly.01",
        "garbage",
    ] {
        assert!(!is_upgrade(invalid, "9.9.9"), "{invalid}");
        assert!(!is_upgrade("0.0.0", invalid), "{invalid}");
    }
}

/// Release consumers delegate ordering here. In particular, Tauri must not
/// silently use its default comparator against the crate or CalVer build ID.
#[test]
fn release_consumers_do_not_reintroduce_independent_comparators() {
    let root = std::path::Path::new(env!("CARGO_MANIFEST_DIR"))
        .parent()
        .unwrap()
        .parent()
        .unwrap();
    for path in [
        "crates/lucidos-gateway/src/release_check.rs",
        "crates/lucidos-gateway/src/update_relay.rs",
        "crates/lucidos-app/src/updater.rs",
    ] {
        let source = std::fs::read_to_string(root.join(path)).unwrap();
        assert!(
            source.contains("is_upgrade("),
            "{path} must use the shared comparator"
        );
        for duplicate in [
            "fn is_upgrade(",
            "fn version_is_newer(",
            "fn is_newer_version(",
            "semver::",
            "Version::parse(",
            ".split('.')",
            ".split(\".\")",
            ".major",
            ".minor",
            ".patch",
            ".cmp(",
            ".partial_cmp(",
        ] {
            assert!(
                !source.contains(duplicate),
                "{path} contains independent version ordering: {duplicate}"
            );
        }
        if path.ends_with("/updater.rs") {
            assert_eq!(source.matches(".version_comparator(").count(), 1);
            assert!(source.contains("is_upgrade(crate::LUCIDOS_RELEASE, &version)"));
            assert!(!source.contains("LUCIDOS_APP_VERSION"));
        }
    }
}
