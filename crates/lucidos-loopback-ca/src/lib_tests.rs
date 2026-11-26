use std::sync::Arc;
use std::time::UNIX_EPOCH;

use rcgen::SanType;
use rustls::client::danger::ServerCertVerifier;
use rustls::client::WebPkiServerVerifier;
use rustls::pki_types::{ServerName, UnixTime};
use rustls::RootCertStore;

use super::*;

pub(crate) fn fresh_ca() -> (tempfile::TempDir, CaDir, LoopbackCa) {
    let scratch = tempfile::tempdir().unwrap();
    let dir = CaDir::at(scratch.path().join("loopback-ca"));
    let ca = LoopbackCa::ensure(&dir).unwrap();
    (scratch, dir, ca)
}

pub(crate) fn verify(
    ca: &LoopbackCa,
    leaf: &CertificateDer<'static>,
    name: &str,
    at: SystemTime,
) -> Result<(), rustls::Error> {
    let mut roots = RootCertStore::empty();
    roots.add(ca.cert_der().clone()).unwrap();
    let provider = Arc::new(rustls::crypto::aws_lc_rs::default_provider());
    let verifier = WebPkiServerVerifier::builder_with_provider(Arc::new(roots), provider)
        .build()
        .unwrap();
    let name = ServerName::try_from(name.to_string()).unwrap();
    let at = UnixTime::since_unix_epoch(at.duration_since(UNIX_EPOCH).unwrap());
    verifier
        .verify_server_cert(leaf, &[], &name, &[], at)
        .map(|_| ())
}

fn mode_of(path: &Path) -> u32 {
    std::fs::metadata(path).unwrap().permissions().mode() & 0o777
}

fn chmod(path: &Path, mode: u32) {
    std::fs::set_permissions(path, std::fs::Permissions::from_mode(mode)).unwrap();
}

#[test]
fn a_new_ca_is_owner_only_and_loads_back_unchanged() {
    let (_scratch, dir, ca) = fresh_ca();
    assert_eq!(mode_of(dir.path()), 0o700);
    assert_eq!(mode_of(&dir.key_path()), 0o600);

    let loaded = LoopbackCa::load(&dir).unwrap();
    assert_eq!(loaded.cert_pem(), ca.cert_pem());
    assert_eq!(dir.read_certificate().unwrap(), ca.cert_pem());
}

#[test]
fn ensuring_publishes_the_certificate_and_no_key_outside_the_private_dir() {
    let (scratch, dir, ca) = fresh_ca();
    assert_eq!(
        dir.public_cert_path(),
        scratch.path().join(PUBLIC_CERT_FILE)
    );
    let copy = std::fs::read_to_string(dir.public_cert_path()).unwrap();
    assert_eq!(copy, ca.cert_pem());
    assert!(!copy.contains("PRIVATE KEY"));
}

#[test]
fn ensuring_repairs_a_stale_public_copy() {
    let (_scratch, dir, ca) = fresh_ca();
    std::fs::write(dir.public_cert_path(), "stale").unwrap();
    LoopbackCa::ensure(&dir).unwrap();
    assert_eq!(dir.read_certificate().unwrap(), ca.cert_pem());
}

#[test]
fn ensuring_twice_keeps_the_first_ca() {
    let (_scratch, dir, ca) = fresh_ca();
    let again = LoopbackCa::ensure(&dir).unwrap();
    assert_eq!(again.cert_pem(), ca.cert_pem());
}

#[test]
fn a_leaf_signed_after_a_reload_still_chains_to_the_ca_on_disk() {
    let (_scratch, dir, ca) = fresh_ca();
    let reloaded = LoopbackCa::load(&dir).unwrap();
    let now = SystemTime::now();
    let leaf = reloaded.mint_leaf(now).unwrap();
    verify(&ca, &leaf.cert_der, "localhost", now).unwrap();
}

#[test]
fn a_key_readable_by_others_is_refused_naming_the_cure() {
    let (_scratch, dir, _ca) = fresh_ca();
    chmod(&dir.key_path(), 0o644);
    let message = LoopbackCa::load(&dir).err().unwrap().to_string();
    assert!(message.contains("mode 644"), "{message}");
    assert!(message.contains("lucidos tls trust"), "{message}");
}

#[test]
fn an_open_directory_is_refused() {
    let (_scratch, dir, _ca) = fresh_ca();
    chmod(dir.path(), 0o755);
    let message = LoopbackCa::load(&dir).err().unwrap().to_string();
    assert!(message.contains("mode 755"), "{message}");
}

#[test]
fn a_ca_owned_by_another_user_is_refused() {
    let (_scratch, dir, _ca) = fresh_ca();
    let me = std::fs::metadata(dir.path()).unwrap().uid();
    let message = check_owner_only_as(&dir, me + 1).unwrap_err().to_string();
    assert!(
        message.contains(&format!("belongs to uid {me}")),
        "{message}"
    );
    assert!(message.contains("lucidos tls trust"), "{message}");
}

#[test]
fn a_certificate_that_does_not_match_the_key_is_refused() {
    let (_scratch, dir, _ca) = fresh_ca();
    let (_other_scratch, _other_dir, other) = fresh_ca();
    std::fs::write(dir.cert_path(), other.cert_pem()).unwrap();
    let message = LoopbackCa::load(&dir).err().unwrap().to_string();
    assert!(message.contains("does not belong to the key"), "{message}");
}

#[test]
fn a_missing_certificate_names_how_to_create_it() {
    let scratch = tempfile::tempdir().unwrap();
    let message = CaDir::at(scratch.path().join("loopback-ca"))
        .read_certificate()
        .unwrap_err()
        .to_string();
    assert!(message.contains("Start Lucidos once"), "{message}");
}

#[test]
fn the_ca_vouches_for_loopback_names_only() {
    let (_scratch, _dir, ca) = fresh_ca();
    let now = SystemTime::now();
    let leaf = ca.mint_leaf(now).unwrap();
    for name in ["localhost", "127.0.0.1", "::1"] {
        verify(&ca, &leaf.cert_der, name, now).unwrap_or_else(|e| panic!("{name}: {e}"));
    }

    let outside = [
        SanType::DnsName("example.com".try_into().unwrap()),
        SanType::IpAddress("192.168.1.10".parse().unwrap()),
    ];
    for (san, name) in outside.into_iter().zip(["example.com", "192.168.1.10"]) {
        let leaf = ca.sign_leaf(vec![san], now).unwrap();
        assert!(
            verify(&ca, &leaf.cert_der, name, now).is_err(),
            "{name} must fall outside the name constraints"
        );
    }
}
