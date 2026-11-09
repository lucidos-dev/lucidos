use super::*;

fn device(id: &str, credential: &str, last_seen_at: Option<&str>) -> PairedDevice {
    PairedDevice {
        id: id.into(),
        label: format!("Device {id}"),
        credential_digest: auth::digest(credential),
        paired_at: "2026-08-17T00:00:00Z".into(),
        last_seen_at: last_seen_at.map(str::to_string),
    }
}

/// Apply `events` live, and append them, the way the gateway does.
fn record(
    log: &EventLog,
    directory: &mut Directory,
    decide: impl FnOnce(&Directory) -> Vec<GatewayEvent>,
) {
    let events = decide(directory);
    let logged: Vec<LoggedEvent> = events.into_iter().map(LoggedEvent::now).collect();
    log.append(&logged).unwrap();
    for event in &logged {
        directory.apply(event);
    }
}

fn reopen(dir: &Path) -> Directory {
    open(dir, None).unwrap().1
}

#[test]
fn replaying_the_log_gives_the_live_state() {
    let dir = tempfile::tempdir().unwrap();
    let (log, mut live, opened) = open(dir.path(), None).unwrap();
    assert_eq!(opened, Opened::Fresh);

    record(&log, &mut live, |d| {
        d.pair("d1", "Phone", "digest-1", None).unwrap()
    });
    let owner = live.first_operator().unwrap().id.clone();
    record(&log, &mut live, |d| d.add_operator("Sam").unwrap());
    let sam = live.operators[1].id.clone();
    record(&log, &mut live, |d| {
        d.pair("d2", "Laptop", "digest-2", Some(&sam)).unwrap()
    });
    record(&log, &mut live, |d| {
        d.add_email(&sam, " Sam@Example.com ").unwrap()
    });
    record(&log, &mut live, |d| d.move_device("d1", &sam).unwrap());
    record(&log, &mut live, |_| {
        vec![GatewayEvent::DeviceSeen {
            device_id: "d2".into(),
        }]
    });
    record(&log, &mut live, |d| {
        d.rename_operator(&owner, "Alex").unwrap()
    });
    record(&log, &mut live, |d| d.revoke("d2"));

    assert_eq!(reopen(dir.path()), live);
    assert_eq!(live.operator_of_device("d1"), Some(sam.as_str()));
    assert_eq!(live.operator_of_email("SAM@example.com").unwrap().id, sam);
    assert!(live.devices.devices.iter().all(|d| d.id != "d2"));
}

#[test]
fn a_first_pairing_creates_the_first_operator_and_later_ones_join_it() {
    // A personal gateway never has to say whose device it is.
    let directory = Directory::default();
    let events = directory.pair("d1", "Phone", "x", None).unwrap();
    assert!(
        matches!(&events[0], GatewayEvent::OperatorAdded { name, .. } if name == FIRST_OPERATOR_NAME)
    );
    let mut directory =
        Directory::replay(&events.into_iter().map(LoggedEvent::now).collect::<Vec<_>>());
    let owner = directory.first_operator().unwrap().id.clone();
    let second = directory.pair("d2", "Laptop", "y", None).unwrap();
    assert_eq!(second.len(), 1, "no second operator is created");
    for event in second {
        directory.apply(&LoggedEvent::now(event));
    }
    assert_eq!(directory.devices_of(&owner), vec!["d1", "d2"]);
}

#[test]
fn a_pairing_for_a_removed_person_is_refused() {
    let directory = Directory::default();
    assert!(directory.pair("d1", "Phone", "x", Some("gone")).is_err());
}

#[test]
fn removing_an_operator_revokes_its_devices_first() {
    let dir = tempfile::tempdir().unwrap();
    let (log, mut live, _) = open(dir.path(), None).unwrap();
    record(&log, &mut live, |d| {
        d.pair("d1", "Phone", "digest-1", None).unwrap()
    });
    record(&log, &mut live, |d| d.add_operator("Sam").unwrap());
    let sam = live.operators[1].id.clone();
    record(&log, &mut live, |d| {
        d.pair("d2", "Laptop", "digest-2", Some(&sam)).unwrap()
    });

    let events = live.remove_operator(&sam).unwrap();
    assert_eq!(
        events,
        vec![
            GatewayEvent::DeviceRevoked {
                device_id: "d2".into()
            },
            GatewayEvent::OperatorRemoved {
                operator_id: sam.clone()
            },
        ]
    );
    record(&log, &mut live, |_| events);
    assert!(live.operator(&sam).is_none());
    assert_eq!(live.devices.devices.len(), 1);
    assert_eq!(reopen(dir.path()), live);
}

#[test]
fn an_email_belongs_to_one_operator() {
    let mut directory = Directory::default();
    for name in ["Sam", "Alex"] {
        for event in directory.add_operator(name).unwrap() {
            directory.apply(&LoggedEvent::now(event));
        }
    }
    let (sam, alex) = (
        directory.operators[0].id.clone(),
        directory.operators[1].id.clone(),
    );
    for event in directory.add_email(&sam, "sam@example.com").unwrap() {
        directory.apply(&LoggedEvent::now(event));
    }
    assert!(directory.add_email(&alex, "SAM@example.com").is_err());
    assert!(directory
        .add_email(&sam, "sam@example.com")
        .unwrap()
        .is_empty());
    assert!(directory.add_email(&sam, "not an email").is_err());
    assert!(directory.add_email(&sam, "sam@localhost").is_err());
}

#[test]
fn the_device_store_migrates_once_into_one_operator() {
    let dir = tempfile::tempdir().unwrap();
    let store = PairedDevices {
        devices: vec![
            device("d1", "cred-1", Some("2026-09-01T00:00:00Z")),
            device("d2", "cred-2", None),
        ],
    };
    store.save(&auth::paired_devices_path(dir.path())).unwrap();

    let (_, directory, opened) = open(dir.path(), None).unwrap();
    assert_eq!(opened, Opened::Migrated(2));
    assert_eq!(
        directory.devices, store,
        "every device and its last-seen survive"
    );
    assert_eq!(directory.operators.len(), 1);
    let owner = &directory.operators[0].id;
    assert_eq!(directory.devices_of(owner), vec!["d1", "d2"]);

    // The store is never read again, so a downgrade cannot revive a revoke.
    assert!(!auth::paired_devices_path(dir.path()).exists());
    let (_, again, opened) = open(dir.path(), None).unwrap();
    assert_eq!(opened, Opened::Replayed);
    assert_eq!(again, directory);
}

#[test]
fn the_legacy_store_seeds_and_is_never_written() {
    let dir = tempfile::tempdir().unwrap();
    let legacy = dir.path().join("legacy.json");
    PairedDevices {
        devices: vec![device("d1", "cred-1", None)],
    }
    .save(&legacy)
    .unwrap();
    let before = std::fs::read_to_string(&legacy).unwrap();
    let gateway = dir.path().join("gw");

    let (_, directory, opened) = open(&gateway, Some(&legacy)).unwrap();
    assert_eq!(opened, Opened::Migrated(1));
    assert_eq!(directory.devices.devices.len(), 1);
    assert_eq!(std::fs::read_to_string(&legacy).unwrap(), before);
}

#[test]
fn a_fresh_install_writes_nothing() {
    let dir = tempfile::tempdir().unwrap();
    let (log, directory, opened) = open(dir.path(), None).unwrap();
    assert_eq!(opened, Opened::Fresh);
    assert_eq!(directory, Directory::default());
    assert!(!log.path().exists());
}

#[test]
fn a_torn_last_line_is_cut_off_and_the_rest_replays() {
    let dir = tempfile::tempdir().unwrap();
    let (log, mut live, _) = open(dir.path(), None).unwrap();
    record(&log, &mut live, |d| {
        d.pair("d1", "Phone", "digest-1", None).unwrap()
    });
    let mut file = open_owner_only(log.path(), false).unwrap();
    file.write_all(br#"{"at":"2026-09-01T00:00:00Z","event":{"type":"DeviceRev"#)
        .unwrap();

    let (log, directory, opened) = open(dir.path(), None).unwrap();
    assert_eq!(opened, Opened::RepairedTornTail);
    assert_eq!(directory, live);
    // The next append starts on a clean line.
    record(&log, &mut live, |d| d.revoke("d1"));
    assert_eq!(reopen(dir.path()), live);
}

#[test]
fn a_failed_append_leaves_no_partial_line_behind() {
    let dir = tempfile::tempdir().unwrap();
    let (log, mut live, _) = open(dir.path(), None).unwrap();
    record(&log, &mut live, |d| {
        d.pair("d1", "Phone", "digest-1", None).unwrap()
    });
    // A full disk: half a line lands, then the write fails.
    let mut file = open_owner_only(log.path(), false).unwrap();
    let failed = append_or_cut(&mut file, |file| {
        file.write_all(br#"{"at":"2026-09-01T00:00:00Z","event":{"type":"DeviceRev"#)?;
        Err(std::io::Error::other("no space left on device"))
    });
    assert!(failed.is_err());

    // The next append lands on a clean line, so the log still replays.
    record(&log, &mut live, |d| d.revoke("d1"));
    let (_, directory, opened) = open(dir.path(), None).unwrap();
    assert_eq!(opened, Opened::Replayed);
    assert_eq!(directory, live);
}

#[test]
fn a_bad_line_in_the_middle_is_an_error_rather_than_a_forgotten_revoke() {
    let dir = tempfile::tempdir().unwrap();
    std::fs::write(
        dir.path().join(LOG_FILE),
        "{ not json\n{\"at\":\"x\",\"event\":{\"type\":\"DeviceSeen\",\"data\":{\"device_id\":\"d1\"}}}\n",
    )
    .unwrap();
    assert!(open(dir.path(), None).is_err());
}

#[test]
fn the_log_never_holds_a_credential() {
    let dir = tempfile::tempdir().unwrap();
    let (log, mut live, _) = open(dir.path(), None).unwrap();
    record(&log, &mut live, |d| {
        d.pair("d1", "Phone", &auth::digest("cred-secret"), None)
            .unwrap()
    });
    let raw = std::fs::read_to_string(log.path()).unwrap();
    assert!(!raw.contains("cred-secret"));
}

#[cfg(unix)]
#[test]
fn the_log_is_owner_only() {
    use std::os::unix::fs::PermissionsExt as _;
    let dir = tempfile::tempdir().unwrap();
    let (log, mut live, _) = open(dir.path(), None).unwrap();
    record(&log, &mut live, |d| {
        d.pair("d1", "Phone", "x", None).unwrap()
    });
    let mode = std::fs::metadata(log.path()).unwrap().permissions().mode() & 0o777;
    assert_eq!(mode, 0o600);
}

#[test]
fn a_relayed_operator_is_recorded_only_when_it_changes() {
    let mut directory = Directory::default();
    let seen = RelayedDevice {
        device_id: "pair-1:d1".into(),
        operator_id: "pair-1:op".into(),
        name: "Sam".into(),
        via: "laptop".into(),
        emails: vec!["sam@example.com".into()],
    };
    let event = directory.relayed_seen(seen.clone()).unwrap();
    directory.apply(&LoggedEvent::now(event));
    assert!(directory.relayed_seen(seen.clone()).is_none());
    let renamed = RelayedDevice {
        name: "Sam K".into(),
        ..seen.clone()
    };
    assert!(directory.relayed_seen(renamed).is_some());
    let new_email = RelayedDevice {
        emails: vec!["sam@example.org".into()],
        ..seen
    };
    assert!(directory.relayed_seen(new_email).is_some());
}

#[test]
fn an_ended_pairing_takes_only_its_own_relayed_people() {
    let mut directory = Directory::default();
    for (device, via) in [("pair-1:d1", "laptop"), ("pair-2:d1", "server")] {
        let seen = RelayedDevice {
            device_id: device.into(),
            operator_id: format!("{via}:op"),
            name: "Sam".into(),
            via: via.into(),
            emails: vec![],
        };
        let event = directory.relayed_seen(seen).unwrap();
        directory.apply(&LoggedEvent::now(event));
    }
    assert_eq!(
        directory.pairing_forgotten("pair-3"),
        None,
        "it relayed nobody"
    );
    let forgotten = directory.pairing_forgotten("pair-1").unwrap();
    assert!(forgotten.changes_identity(), "engines hear of it");
    directory.apply(&LoggedEvent::now(forgotten));
    let left: Vec<&str> = directory
        .relayed
        .iter()
        .map(|r| r.device_id.as_str())
        .collect();
    assert_eq!(left, vec!["pair-2:d1"]);
}

#[test]
fn only_a_seen_stamp_leaves_identity_alone() {
    assert!(!GatewayEvent::DeviceSeen {
        device_id: "d".into()
    }
    .changes_identity());
    assert!(GatewayEvent::DeviceRevoked {
        device_id: "d".into()
    }
    .changes_identity());
}
