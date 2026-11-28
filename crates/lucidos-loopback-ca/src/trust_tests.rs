use std::cell::RefCell;
use std::os::unix::process::ExitStatusExt;
use std::process::ExitStatus;

use super::*;
use crate::LoopbackCa;

struct Fake<F: Fn(&Argv) -> io::Result<Output>> {
    ran: RefCell<Vec<Argv>>,
    answer: F,
}

impl<F: Fn(&Argv) -> io::Result<Output>> Run for Fake<F> {
    fn run(&self, argv: &Argv) -> io::Result<Output> {
        self.ran.borrow_mut().push(argv.clone());
        (self.answer)(argv)
    }
}

fn fake<F: Fn(&Argv) -> io::Result<Output>>(answer: F) -> Fake<F> {
    Fake {
        ran: RefCell::new(Vec::new()),
        answer,
    }
}

fn output(code: i32, stdout: &str, stderr: &str) -> io::Result<Output> {
    Ok(Output {
        status: ExitStatus::from_raw(code << 8),
        stdout: stdout.as_bytes().to_vec(),
        stderr: stderr.as_bytes().to_vec(),
    })
}

fn missing() -> io::Result<Output> {
    Err(io::Error::from(io::ErrorKind::NotFound))
}

fn first_arg(argv: &Argv) -> String {
    argv.args[0].to_string_lossy().into_owned()
}

struct Setup {
    _scratch: tempfile::TempDir,
    ca_dir: CaDir,
    pem: String,
    stores: Stores,
}

fn setup(platform: Platform) -> Setup {
    let scratch = tempfile::tempdir().unwrap();
    let ca_dir = CaDir::at(scratch.path().join("ca"));
    let pem = LoopbackCa::ensure(&ca_dir).unwrap().cert_pem().to_string();
    let home = scratch.path().join("home");
    let firefox = home.join(".mozilla/firefox");
    for db in [home.join(NSS_USER_DB), firefox.join("abc.default")] {
        std::fs::create_dir_all(&db).unwrap();
        std::fs::write(db.join("cert9.db"), "").unwrap();
    }
    std::fs::write(
        firefox.join("profiles.ini"),
        "[Install4F96D1932A9F858E]\nDefault=abc.default\n\n\
         [Profile0]\nName=default\nIsRelative=1\nPath=abc.default\n",
    )
    .unwrap();
    Setup {
        _scratch: scratch,
        ca_dir,
        pem,
        stores: Stores { platform, home },
    }
}

fn every_arg(ran: &[Argv]) -> Vec<String> {
    ran.iter()
        .flat_map(|a| a.args.iter().map(|s| s.to_string_lossy().into_owned()))
        .collect()
}

#[test]
fn macos_trusts_in_the_login_keychain_user_domain_only() {
    let s = setup(Platform::MacOs);
    let run = fake(|argv| match first_arg(argv).as_str() {
        "login-keychain" => output(
            0,
            "    \"/Users/me/Library/Keychains/login.keychain-db\"\n",
            "",
        ),
        _ => output(0, "", ""),
    });
    trust_with(&run, &s.stores, &s.ca_dir).unwrap();
    untrust_with(&run, &s.stores, &s.ca_dir).unwrap();

    let ran = run.ran.borrow();
    let add = ran
        .iter()
        .find(|a| first_arg(a) == "add-trusted-cert")
        .unwrap();
    assert!(add.args.contains(&OsString::from(
        "/Users/me/Library/Keychains/login.keychain-db"
    )));
    let args = every_arg(&ran);
    assert!(!args.iter().any(|a| a == "-d"), "{args:?}");
    assert!(
        !args.iter().any(|a| a.contains("System.keychain")),
        "{args:?}"
    );
}

#[test]
fn ensure_trusted_skips_the_prompt_when_the_keychain_already_trusts() {
    let s = setup(Platform::MacOs);
    let run = fake(|_| output(0, "", ""));
    ensure_trusted_with(&run, &s.stores, &s.ca_dir).unwrap();
    let firsts: Vec<String> = run.ran.borrow().iter().map(first_arg).collect();
    assert_eq!(firsts, ["verify-cert"]);
}

#[test]
fn ensure_trusted_re_adds_on_linux_whose_status_cannot_see_trust_flags() {
    let s = setup(Platform::Linux);
    let pem = s.pem.clone();
    let run = fake(move |argv| match first_arg(argv).as_str() {
        "-L" => output(0, &pem, ""),
        _ => output(0, "", ""),
    });
    ensure_trusted_with(&run, &s.stores, &s.ca_dir).unwrap();
    assert!(run.ran.borrow().iter().any(|a| first_arg(a) == "-A"));
}

#[test]
fn ensure_trusted_trusts_an_untrusted_ca() {
    let s = setup(Platform::MacOs);
    let run = fake(|argv| match first_arg(argv).as_str() {
        "login-keychain" => output(0, "\"/k\"", ""),
        "verify-cert" => output(1, "", "CSSMERR_TP_NOT_TRUSTED"),
        _ => output(0, "", ""),
    });
    ensure_trusted_with(&run, &s.stores, &s.ca_dir).unwrap();
    assert!(run
        .ran
        .borrow()
        .iter()
        .any(|a| first_arg(a) == "add-trusted-cert"));
}

#[test]
fn macos_status_is_the_keychains_own_verdict() {
    let s = setup(Platform::MacOs);
    let trusted = fake(|_| output(0, "", ""));
    assert_eq!(
        status_with(&trusted, &s.stores, &s.ca_dir),
        TrustStatus::Trusted
    );
    assert_eq!(first_arg(&trusted.ran.borrow()[0]), "verify-cert");

    let untrusted = fake(|_| output(1, "", "CSSMERR_TP_NOT_TRUSTED"));
    assert!(matches!(
        status_with(&untrusted, &s.stores, &s.ca_dir),
        TrustStatus::Untrusted(_)
    ));
}

#[test]
fn a_cancelled_prompt_names_the_retry() {
    let s = setup(Platform::MacOs);
    let run = fake(|argv| match first_arg(argv).as_str() {
        "login-keychain" => output(0, "\"/k\"", ""),
        _ => output(1, "", "The authorization was canceled by the user."),
    });
    let message = trust_with(&run, &s.stores, &s.ca_dir)
        .unwrap_err()
        .to_string();
    assert!(message.contains("canceled by the user"), "{message}");
    assert!(message.contains("lucidos tls trust"), "{message}");
}

#[test]
fn linux_trusts_the_user_database_and_every_firefox_profile() {
    let s = setup(Platform::Linux);
    let run = fake(|_| output(0, "", ""));
    trust_with(&run, &s.stores, &s.ca_dir).unwrap();

    let ran = run.ran.borrow();
    let added: Vec<String> = ran
        .iter()
        .filter(|a| first_arg(a) == "-A")
        .map(|a| a.args[2].to_string_lossy().into_owned())
        .collect();
    let home = s.stores.home.display().to_string();
    assert_eq!(
        added,
        [
            format!("sql:{home}/{NSS_USER_DB}"),
            format!("sql:{home}/.mozilla/firefox/abc.default"),
        ]
    );
    let args = every_arg(&ran);
    assert!(!args.iter().any(|a| a.contains("/etc")), "{args:?}");
}

#[test]
fn linux_creates_the_user_database_when_chrome_never_did() {
    let s = setup(Platform::Linux);
    std::fs::remove_dir_all(s.stores.home.join(NSS_USER_DB)).unwrap();
    let home = s.stores.home.clone();
    let run = fake(move |argv| {
        if first_arg(argv) == "-N" {
            std::fs::write(home.join(NSS_USER_DB).join("cert9.db"), "").unwrap();
        }
        output(0, "", "")
    });
    trust_with(&run, &s.stores, &s.ca_dir).unwrap();
    let firsts: Vec<String> = run.ran.borrow().iter().map(first_arg).collect();
    assert_eq!(firsts[0], "-N");
    assert_eq!(firsts.iter().filter(|f| *f == "-A").count(), 2);
}

#[test]
fn linux_status_needs_the_ca_in_every_database() {
    let s = setup(Platform::Linux);
    let pem = s.pem.clone();
    let everywhere = fake(move |_| output(0, &pem, ""));
    assert_eq!(
        status_with(&everywhere, &s.stores, &s.ca_dir),
        TrustStatus::Trusted
    );

    let pem = s.pem.clone();
    let only_chrome = fake(move |argv| {
        if argv.args[2].to_string_lossy().contains("firefox") {
            output(255, "", "could not find certificate named")
        } else {
            output(0, &pem, "")
        }
    });
    let TrustStatus::Untrusted(why) = status_with(&only_chrome, &s.stores, &s.ca_dir) else {
        panic!("a profile without the CA is not trusted");
    };
    assert!(why.contains("abc.default"), "{why}");
}

#[test]
fn a_missing_certutil_names_the_package() {
    let s = setup(Platform::Linux);
    let run = fake(|_| missing());
    assert_eq!(
        status_with(&run, &s.stores, &s.ca_dir),
        TrustStatus::Unavailable(CERTUTIL_MISSING.to_string())
    );
    let message = trust_with(&run, &s.stores, &s.ca_dir)
        .unwrap_err()
        .to_string();
    assert_eq!(message, CERTUTIL_MISSING);
    assert!(message.contains("libnss3-tools"));
}

#[test]
fn profiles_ini_names_relative_and_absolute_profiles() {
    let scratch = tempfile::tempdir().unwrap();
    std::fs::write(
        scratch.path().join("profiles.ini"),
        "[General]\nStartWithLastProfile=1\n\n\
         [Profile1]\nPath=/opt/elsewhere\nIsRelative=0\n\n\
         [Profile0]\nIsRelative=1\nPath=x.default-release\n\n\
         [Install1]\nDefault=x.default-release\n",
    )
    .unwrap();
    assert_eq!(
        linux::firefox_profiles(scratch.path()),
        [
            PathBuf::from("/opt/elsewhere"),
            scratch.path().join("x.default-release"),
        ]
    );
}

#[test]
fn a_listing_holds_the_certificate_whatever_its_wrapping() {
    let pem = "-----BEGIN CERTIFICATE-----\nAAAA\nBBBB\n-----END CERTIFICATE-----\n";
    let rewrapped = "-----BEGIN CERTIFICATE-----\r\nAAAABBBB\r\n-----END CERTIFICATE-----";
    assert!(holds(rewrapped.as_bytes(), pem));
    assert!(!holds(b"", pem));
}

fn mac(found: String, remove: i32, verify: i32) -> impl Fn(&Argv) -> io::Result<Output> {
    move |argv| match first_arg(argv).as_str() {
        "login-keychain" => output(0, "\"/Users/me/login.keychain-db\"", ""),
        "find-certificate" if found.is_empty() => output(44, "", "not found"),
        "find-certificate" => output(0, &found, ""),
        "remove-trusted-cert" => output(remove, "", "could not be found"),
        "verify-cert" => output(verify, "", ""),
        _ => output(0, "", ""),
    }
}

#[test]
fn macos_untrusts_a_ca_it_never_trusted() {
    let s = setup(Platform::MacOs);
    let run = fake(mac(String::new(), 1, 1));
    untrust_with(&run, &s.stores, &s.ca_dir).unwrap();
    let firsts: Vec<String> = run.ran.borrow().iter().map(first_arg).collect();
    assert!(
        !firsts.contains(&"delete-certificate".to_string()),
        "{firsts:?}"
    );
}

#[test]
fn macos_untrust_fails_while_the_ca_stays_trusted() {
    let s = setup(Platform::MacOs);
    let run = fake(mac(String::new(), 1, 0));
    assert!(untrust_with(&run, &s.stores, &s.ca_dir).is_err());
}

#[test]
fn macos_untrust_deletes_the_certificate_the_keychain_holds() {
    let s = setup(Platform::MacOs);
    let digest = fingerprint(&s.pem).unwrap();
    let run = fake(mac(format!("SHA-256 hash: {digest}\n"), 0, 1));
    untrust_with(&run, &s.stores, &s.ca_dir).unwrap();
    let ran = run.ran.borrow();
    let delete = ran
        .iter()
        .find(|a| first_arg(a) == "delete-certificate")
        .expect("the held certificate is deleted");
    assert!(delete.args.contains(&OsString::from(digest)));
}

#[test]
fn forget_untrusts_then_deletes_the_ca_and_its_public_copy() {
    let s = setup(Platform::Linux);
    let run = fake(|_| output(0, "", ""));
    assert!(forget_with(&run, &s.stores, &s.ca_dir).unwrap());
    assert!(run.ran.borrow().iter().any(|a| first_arg(a) == "-D"));
    assert!(!s.ca_dir.path().exists());
    assert!(!s.ca_dir.public_cert_path().exists());
}

#[test]
fn forget_untrusts_from_the_private_certificate_when_the_public_copy_is_gone() {
    let s = setup(Platform::MacOs);
    std::fs::remove_file(s.ca_dir.public_cert_path()).unwrap();
    let digest = fingerprint(&s.pem).unwrap();
    let run = fake(mac(format!("SHA-256 hash: {digest}\n"), 0, 1));
    assert!(forget_with(&run, &s.stores, &s.ca_dir).unwrap());
    let firsts: Vec<String> = run.ran.borrow().iter().map(first_arg).collect();
    assert!(
        firsts.contains(&"delete-certificate".to_string()),
        "{firsts:?}"
    );
    assert!(!s.ca_dir.path().exists());
}

#[test]
fn forget_keeps_the_ca_when_untrust_fails() {
    let s = setup(Platform::MacOs);
    let run = fake(mac(String::new(), 1, 0));
    assert!(forget_with(&run, &s.stores, &s.ca_dir).is_err());
    assert!(s.ca_dir.path().exists());
}

#[test]
fn forget_without_a_ca_runs_nothing() {
    let s = setup(Platform::Linux);
    let absent = CaDir::at(s.stores.home.join("no-ca"));
    let run = fake(|_| output(0, "", ""));
    assert!(!forget_with(&run, &s.stores, &absent).unwrap());
    assert!(run.ran.borrow().is_empty());
}

#[test]
fn linux_untrust_deletes_only_where_the_ca_is() {
    let s = setup(Platform::Linux);
    let run = fake(|argv| match first_arg(argv).as_str() {
        "-L" if argv.args[2].to_string_lossy().contains("firefox") => {
            output(255, "", "could not find certificate named")
        }
        _ => output(0, "", ""),
    });
    untrust_with(&run, &s.stores, &s.ca_dir).unwrap();
    let deleted: Vec<String> = run
        .ran
        .borrow()
        .iter()
        .filter(|a| first_arg(a) == "-D")
        .map(|a| a.args[2].to_string_lossy().into_owned())
        .collect();
    let home = s.stores.home.display().to_string();
    assert_eq!(deleted, [format!("sql:{home}/{NSS_USER_DB}")]);
}

#[test]
fn linux_reaches_the_snap_chromium_and_xdg_firefox_databases() {
    let s = setup(Platform::Linux);
    let home = &s.stores.home;
    let xdg = home.join(".config/mozilla/firefox");
    for db in [home.join(SNAP_CHROMIUM_DB), xdg.join("xyz.default")] {
        std::fs::create_dir_all(&db).unwrap();
        std::fs::write(db.join("cert9.db"), "").unwrap();
    }
    std::fs::write(
        xdg.join("profiles.ini"),
        "[Profile0]\nIsRelative=1\nPath=xyz.default\n",
    )
    .unwrap();
    let dbs = linux::databases(home);
    assert!(dbs.contains(&home.join(SNAP_CHROMIUM_DB)), "{dbs:?}");
    assert!(dbs.contains(&xdg.join("xyz.default")), "{dbs:?}");
}
