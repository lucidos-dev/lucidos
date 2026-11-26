use super::*;
use lucidos_installs::{Install, InstallKind, Inventory, PortConflict};

const APP: &str = "Lucidos.app in /Applications";
const INSTANCE: &str = "install.sh instance \"default\"";

fn install(kind: InstallKind, name: &str, version: Option<&str>, port: u16) -> Install {
    Install {
        kind,
        name: name.to_string(),
        root: None,
        version: version.map(str::to_string),
        data_dir: None,
        port: Some(port),
        agents: Vec::new(),
        running_here: false,
        removal: "removal command".to_string(),
    }
}

fn contended() -> Inventory {
    Inventory {
        installs: vec![
            install(InstallKind::DesktopApp, APP, Some("0.36.0"), 5252),
            install(
                InstallKind::HeadlessInstaller,
                INSTANCE,
                Some("0.26.2"),
                5252,
            ),
        ],
        conflicts: vec![PortConflict {
            port: 5252,
            installs: vec![APP.to_string(), INSTANCE.to_string()],
        }],
    }
}

fn coexisting() -> Inventory {
    Inventory {
        installs: vec![
            install(InstallKind::DesktopApp, APP, Some("1.2.3"), 5252),
            install(InstallKind::SourceCheckout, "Source checkout", None, 5251),
        ],
        conflicts: Vec::new(),
    }
}

#[test]
fn coexisting_installs_produce_no_notice() {
    assert_eq!(
        conflict_notice(&coexisting(), 5252, Some("1.2.3"), "1.2.3"),
        None,
        "two installs on two ports are a supported setup, and must never warn"
    );
}

#[test]
fn a_conflict_on_another_port_is_not_this_client_s_problem() {
    let mut inv = coexisting();
    inv.conflicts.push(PortConflict {
        port: 5300,
        installs: vec!["one".to_string(), "two".to_string()],
    });

    assert_eq!(
        conflict_notice(&inv, 5252, None, "1.2.3"),
        None,
        "the dialog is about the port this client connects to"
    );
}

#[test]
fn a_contended_port_names_both_installs_and_their_versions() {
    let notice = conflict_notice(&contended(), 5252, None, "0.36.0").expect("a conflict speaks");

    assert!(notice.body.contains("port 5252"));
    assert!(notice.body.contains(APP));
    assert!(notice.body.contains("version 0.36.0"));
    assert!(notice.body.contains(INSTANCE));
    assert!(notice.body.contains("version 0.26.2"));
    assert!(notice.body.contains("Overview"), "and where to read more");
}

#[test]
fn the_notice_names_the_version_actually_answering() {
    let notice = conflict_notice(&contended(), 5252, Some("0.26.2"), "0.36.0").unwrap();

    assert!(
        notice
            .body
            .contains("port 5252 is answered by Lucidos 0.26.2"),
        "the live version is the fact nothing in the product used to say: {}",
        notice.body
    );
    assert!(notice.body.contains("this app is 0.36.0"));
}

#[test]
fn a_matching_release_leaves_the_live_version_unsaid() {
    let notice = conflict_notice(&contended(), 5252, Some("0.36.0"), "0.36.0").unwrap();

    assert!(!notice.body.contains("is answered by"));
}

#[test]
fn an_install_with_no_readable_version_says_so() {
    let mut inv = contended();
    inv.installs[1].version = None;

    let notice = conflict_notice(&inv, 5252, None, "0.36.0").unwrap();

    assert!(notice.body.contains("version unknown"));
}

#[test]
fn the_notice_is_said_once_per_machine_state() {
    let first = conflict_notice(&contended(), 5252, None, "0.36.0").unwrap();
    assert!(
        should_announce(&first.fingerprint, None),
        "nothing said yet"
    );
    assert!(
        !should_announce(&first.fingerprint, Some(&first.fingerprint)),
        "the same conflict is not worth a second dialog"
    );

    let mut grown = contended();
    grown.conflicts[0].installs.push("a third".to_string());
    let second = conflict_notice(&grown, 5252, None, "0.36.0").unwrap();
    assert!(
        should_announce(&second.fingerprint, Some(&first.fingerprint)),
        "a different conflict is a different thing to say"
    );
}

#[test]
fn an_empty_fingerprint_announces_nothing() {
    assert!(!should_announce("", None));
}

#[test]
fn parse_release_tolerates_a_gateway_that_carries_no_version() {
    assert_eq!(
        parse_release(r#"{"status":"ok","role":"gateway","release":"0.26.2"}"#).as_deref(),
        Some("0.26.2")
    );
    assert_eq!(
        parse_release(r#"{"release":"9.2.0-rc.10"}"#).as_deref(),
        Some("9.2.0-rc.10")
    );
    assert_eq!(parse_release(r#"{"status":"ok"}"#), None);
    assert_eq!(parse_release(r#"{"release":"  "}"#), None);
    assert_eq!(parse_release("<!DOCTYPE html>"), None);
    assert_eq!(parse_release(""), None);
}
