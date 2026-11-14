//! Whether a URL's path stays under a configured path prefix.
//!
//! The one place a path scope is judged. `api::proxy` asks it before it
//! forwards a request with its credentials. `core::credentials` asks it before
//! a stored credential is injected into a request.
//!
//! The URL parser resolves `..` and `%2e%2e` segments and turns `\` into `/`.
//! It leaves every other escape alone, such as `%2f`, `%5c` and `%25`. An
//! upstream stack can percent-decode the path again, once or several times,
//! and then reads a path the parser never saw. So the scope must hold for
//! every reading, not just the first. Each reading is a full decode, because a
//! blocklist of escape spellings holds only until the next spelling.

use reqwest::Url;

/// How many decode layers the probe chases before it refuses.
/// Real upstreams decode once or twice; the cap bounds what one request costs.
pub(crate) const MAX_DECODE_ROUNDS: usize = 8;

/// Why a path is out of scope.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) enum PathEscape {
    /// The path as parsed already leaves the prefix.
    Parsed(String),
    /// A deeper decoded reading leaves the prefix. Holds that reading's path.
    Decoded(String),
    /// The path still changes after [`MAX_DECODE_ROUNDS`] rounds.
    TooDeep,
}

/// `Ok` when `target`'s path, and every deeper decoded reading of it, is
/// `base_path` itself or sits under it.
///
/// `base_path` carries no trailing slash. An empty one means no prefix, so
/// nothing is out of scope. The caller checks the origin.
pub(crate) fn check_path_within(base_path: &str, target: &Url) -> Result<(), PathEscape> {
    if base_path.is_empty() {
        return Ok(());
    }
    if !path_is_within(base_path, target.path()) {
        return Err(PathEscape::Parsed(target.path().to_string()));
    }
    // Decode only what follows the prefix. The prefix is compared as written,
    // so decoding it too would refuse a scope like `/projects/group%2Fproj`.
    let Some(suffix) = target.path().strip_prefix(base_path) else {
        return Err(PathEscape::Parsed(target.path().to_string()));
    };
    let readings = decoded_readings(suffix).ok_or(PathEscape::TooDeep)?;
    for reading in readings {
        // `set_path` runs the parser's path rules again, so a decoded `..`,
        // `%2e%2e` or `\` pops a segment exactly as an upstream would. It also
        // re-escapes a decoded `?` or `#`, which keeps them inside the path.
        let mut probe = target.clone();
        probe.set_path(&format!("{base_path}{reading}"));
        if !path_is_within(base_path, probe.path()) {
            return Err(PathEscape::Decoded(probe.path().to_string()));
        }
    }
    Ok(())
}

/// True when `target_path` is `base_path` itself or sits under it.
///
/// An empty `base_path` means no prefix, so nothing is out of bounds. The
/// boundary is a segment, never a string prefix, so `/safe-prefix-evil` does
/// not count as inside `/safe-prefix`.
pub(crate) fn path_is_within(base_path: &str, target_path: &str) -> bool {
    base_path.is_empty()
        || target_path == base_path
        || target_path
            .strip_prefix(base_path)
            .is_some_and(|rest| rest.starts_with('/'))
}

/// Every deeper reading of `path`, one per decode layer. For the containment
/// probe, never for sending.
///
/// A round is one full percent-decode. Like a lenient upstream decoder, it
/// keeps an escape that is not valid as written. So `%252f` becomes `%2f`,
/// then `/`, and `%2%65` becomes `%2e`, then `.`. An upstream stack that
/// decodes twice is covered as well as one that decodes once.
///
/// `None` means the path still changes after [`MAX_DECODE_ROUNDS`] rounds, and
/// the caller refuses it. One round can peel a single `%25`, so an uncapped
/// probe costs time and memory quadratic in the path length. Refusing past the
/// cap, rather than accepting, keeps a deeper nesting from becoming a bypass.
fn decoded_readings(path: &str) -> Option<Vec<String>> {
    let mut readings: Vec<String> = Vec::new();
    loop {
        let current = readings.last().map_or(path, String::as_str);
        let next =
            String::from_utf8_lossy(&urlencoding::decode_binary(current.as_bytes())).into_owned();
        if next == current {
            return Some(readings);
        }
        if readings.len() == MAX_DECODE_ROUNDS {
            return None;
        }
        readings.push(next);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn check(base_path: &str, url: &str) -> Result<(), PathEscape> {
        check_path_within(base_path, &Url::parse(url).unwrap())
    }

    #[test]
    fn decoded_readings_peels_one_layer_per_round_and_terminates() {
        assert_eq!(
            decoded_readings("%2e%2e%252fadmin"),
            Some(vec!["..%2fadmin".to_string(), "../admin".to_string()])
        );
        assert_eq!(
            decoded_readings("..%5cadmin"),
            Some(vec!["..\\admin".to_string()])
        );
        // An escape that is not valid as written survives the round it is in.
        assert_eq!(
            decoded_readings("%2%65"),
            Some(vec!["%2e".to_string(), ".".to_string()])
        );
        // Nothing to decode means no extra probe to run.
        assert_eq!(decoded_readings("items/42"), Some(Vec::new()));
    }

    /// Regression: one round peels a single `%25`, so `%` followed by `25` k
    /// times took k rounds and kept every reading. A path near the URI limit
    /// held about a gigabyte of strings, and a few such requests could OOM the
    /// engine.
    #[test]
    fn a_deeply_nested_path_is_refused_without_peeling_every_layer() {
        // Each `25` after the `%` is one more layer, and the final `%2f` takes
        // one more round to become `/`.
        let at_cap = format!("%{}2fadmin", "25".repeat(MAX_DECODE_ROUNDS - 1));
        assert_eq!(
            decoded_readings(&at_cap).map(|r| r.len()),
            Some(MAX_DECODE_ROUNDS),
            "a path exactly at the cap is still probed layer by layer"
        );

        let past_cap = format!("%{}2fadmin", "25".repeat(MAX_DECODE_ROUNDS));
        assert_eq!(decoded_readings(&past_cap), None);

        let hostile = format!("%{}", "25".repeat(30_000));
        assert_eq!(decoded_readings(&hostile), None);
        assert_eq!(
            check("/safe", &format!("https://h.test/safe/{hostile}")),
            Err(PathEscape::TooDeep)
        );
    }

    #[test]
    fn a_decoded_reading_that_pops_the_prefix_is_named() {
        assert_eq!(
            check("/safe", "https://h.test/safe/%252e%252e%252fadmin"),
            Err(PathEscape::Decoded("/admin".to_string()))
        );
        assert_eq!(
            check("/safe", "https://h.test/safe/%2%65%2%65/admin"),
            Err(PathEscape::Decoded("/admin".to_string()))
        );
        assert_eq!(
            check("/safe", "https://h.test/safe/..%255cadmin"),
            Err(PathEscape::Decoded("/admin".to_string()))
        );
        assert_eq!(
            check("/safe", "https://h.test/safe-evil/x"),
            Err(PathEscape::Parsed("/safe-evil/x".to_string()))
        );
    }

    #[test]
    fn an_encoded_slash_that_stays_inside_the_prefix_passes() {
        assert_eq!(check("/safe", "https://h.test/safe/a%2fb"), Ok(()));
        assert_eq!(check("/safe", "https://h.test/safe"), Ok(()));
        assert_eq!(check("/safe", "https://h.test/safe/a%20b%3fc"), Ok(()));
    }

    /// Regression: decoding the prefix too refused every request under a scope
    /// whose own path holds an encoded slash.
    #[test]
    fn a_prefix_with_an_encoded_slash_is_compared_as_written() {
        let base = "/api/projects/group%2Fproj";
        assert_eq!(
            check(base, "https://h.test/api/projects/group%2Fproj/issues"),
            Ok(())
        );
        assert_eq!(
            check(
                base,
                "https://h.test/api/projects/group%2Fproj/%2e%2e%2fother"
            ),
            Err(PathEscape::Decoded("/api/projects/other".to_string()))
        );
        // No prefix means nothing is out of scope.
        assert_eq!(check("", "https://h.test/%2e%2e%2fadmin"), Ok(()));
    }
}
