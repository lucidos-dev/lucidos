//! This node's TLS certificate, fetched from tailscaled.
//!
//! tailscaled gets the certificate from its control server, which issues one
//! only for the node's own name. So the caller passes the name tailscaled
//! itself reported ([`crate::TailnetNode::https_name`]), and this module never
//! builds or guesses one.

use std::time::Duration;

use crate::discovery::{discover, Endpoint, Platform};
use crate::{localapi, Answer, SystemProbes};

/// A first issuance waits on the control server's ACME order, which can take
/// tens of seconds. A cached certificate comes back at once.
const CERT_TIMEOUT: Duration = Duration::from_secs(90);

const CERT_BEGIN: &str = "-----BEGIN CERTIFICATE-----";

/// A certificate chain and its private key, both PEM.
pub struct CertPair {
    pub cert_pem: Vec<u8>,
    pub key_pem: Vec<u8>,
}

/// Ask tailscaled for the certificate of `name`, this node's own name.
/// Blocking, for up to a minute and a half; call it off any async worker.
/// `Err` is one sentence for the user on what went wrong.
pub fn cert_pair(name: &str) -> Result<CertPair, String> {
    let found = discover(Platform::current(), &SystemProbes);
    fetch(&found.endpoints, name, |endpoint, path| {
        localapi::get(endpoint, path, CERT_TIMEOUT)
    })
}

fn fetch(
    endpoints: &[Endpoint],
    name: &str,
    get: impl Fn(&Endpoint, &str) -> Answer,
) -> Result<CertPair, String> {
    if !is_dns_name(name) {
        return Err(format!(
            "{name:?} is not a DNS name tailscaled can certify."
        ));
    }
    let path = format!("/localapi/v0/cert/{name}?type=pair");
    let mut silent = false;
    for endpoint in endpoints {
        match get(endpoint, &path) {
            Answer::Body(body) => return split_pair(&body),
            Answer::Refused { status_line, body } => return Err(refusal(&status_line, &body)),
            Answer::Garbled(detail) => {
                return Err(format!(
                    "tailscaled's certificate reply was unreadable: {detail}"
                ))
            }
            Answer::Silent => silent = true,
        }
    }
    Err(if silent {
        "tailscaled did not answer the certificate request.".into()
    } else {
        "Lucidos found no tailscaled to ask for a certificate.".into()
    })
}

/// tailscaled's own reason, plus the fix when it is a permission refusal.
fn refusal(status_line: &str, body: &str) -> String {
    let reason = match body.trim() {
        "" => status_line,
        said => said,
    };
    let mut sentence = format!("tailscaled refused the certificate: {reason}.");
    if status_line.split_whitespace().nth(1) == Some("403") {
        sentence.push_str(
            " tailscaled hands certificates only to root and its operator user. \
             Make this user the operator with `sudo tailscale set --operator=$USER`.",
        );
    }
    sentence
}

/// `type=pair` answers with the key PEM, then the certificate chain PEM.
fn split_pair(body: &str) -> Result<CertPair, String> {
    let split = body
        .find(CERT_BEGIN)
        .ok_or("tailscaled's reply held no certificate.")?;
    let (key, cert) = body.split_at(split);
    if !key.contains("PRIVATE KEY-----") {
        return Err("tailscaled's reply held no private key.".into());
    }
    Ok(CertPair {
        cert_pem: cert.trim().as_bytes().to_vec(),
        key_pem: key.trim().as_bytes().to_vec(),
    })
}

/// Letters, digits, hyphens and dots only: the name goes into a URL path.
fn is_dns_name(name: &str) -> bool {
    !name.is_empty()
        && name.len() <= 253
        && name
            .bytes()
            .all(|b| b.is_ascii_alphanumeric() || b == b'-' || b == b'.')
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::cell::RefCell;

    const KEY: &str = "-----BEGIN EC PRIVATE KEY-----\nAAAA\n-----END EC PRIVATE KEY-----\n";
    const CERT: &str = "-----BEGIN CERTIFICATE-----\nBBBB\n-----END CERTIFICATE-----\n";

    fn socket() -> Endpoint {
        Endpoint::Unix("/var/run/tailscale/tailscaled.sock".into())
    }

    fn reply(answer: impl Fn() -> Answer) -> impl Fn(&Endpoint, &str) -> Answer {
        move |_, _| answer()
    }

    #[test]
    fn it_asks_for_exactly_the_name_it_was_given() {
        let asked = RefCell::new(Vec::new());
        let pair = fetch(&[socket()], "node-7.some-base.example", |_, path| {
            asked.borrow_mut().push(path.to_string());
            Answer::Body(format!("{KEY}{CERT}"))
        })
        .unwrap();
        assert_eq!(
            asked.into_inner(),
            ["/localapi/v0/cert/node-7.some-base.example?type=pair"]
        );
        assert!(String::from_utf8(pair.key_pem)
            .unwrap()
            .contains("PRIVATE KEY"));
        assert!(String::from_utf8(pair.cert_pem)
            .unwrap()
            .starts_with(CERT_BEGIN));
    }

    #[test]
    fn a_name_that_could_change_the_path_is_never_requested() {
        for name in ["", "a/b", "a?type=x", "a b", "../status"] {
            let result = fetch(&[socket()], name, |_, _| unreachable!("asked for {name:?}"));
            assert!(result.is_err());
        }
    }

    #[test]
    fn a_permission_refusal_names_the_fix() {
        let err = fetch(
            &[socket()],
            "n.example",
            reply(|| Answer::Refused {
                status_line: "HTTP/1.0 403 Forbidden".into(),
                body: "cert access denied\n".into(),
            }),
        )
        .err()
        .unwrap();
        assert!(err.contains("cert access denied"), "{err}");
        assert!(err.contains("--operator"), "{err}");
    }

    #[test]
    fn another_refusal_carries_tailscaleds_reason_alone() {
        let err = fetch(
            &[socket()],
            "n.example",
            reply(|| Answer::Refused {
                status_line: "HTTP/1.0 500 Internal Server Error".into(),
                body: "your Tailscale account does not support getting TLS certs".into(),
            }),
        )
        .err()
        .unwrap();
        assert!(err.contains("does not support getting TLS certs"), "{err}");
        assert!(!err.contains("--operator"), "{err}");
    }

    #[test]
    fn a_silent_endpoint_gives_way_to_the_next() {
        let calls = RefCell::new(0);
        let pair = fetch(&[socket(), socket()], "n.example", |_, _| {
            *calls.borrow_mut() += 1;
            if *calls.borrow() == 1 {
                Answer::Silent
            } else {
                Answer::Body(format!("{KEY}{CERT}"))
            }
        });
        assert!(pair.is_ok());
    }

    #[test]
    fn nothing_to_ask_and_nothing_answering_are_told_apart() {
        let none = fetch(&[], "n.example", reply(|| unreachable!()));
        let silent = fetch(&[socket()], "n.example", reply(|| Answer::Silent));
        assert!(none.err().unwrap().contains("found no tailscaled"));
        assert!(silent.err().unwrap().contains("did not answer"));
    }

    #[test]
    fn a_reply_missing_either_half_is_an_error() {
        assert!(split_pair(KEY).is_err());
        assert!(split_pair(CERT).is_err());
        assert!(split_pair(&format!("{KEY}{CERT}")).is_ok());
    }
}
