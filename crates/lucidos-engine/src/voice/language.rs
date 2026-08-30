//! The Locale language, as something one call can be pinned to.
//!
//! A call needs two different things from one preference. The transcriber wants
//! an ISO-639-1 code, and the talker wants a name to speak in. The preference
//! is free text, so only the second is always available.
//!
//! **An unresolved name never makes a call worse.** The code stays `None`, the
//! transcriber payload is what it was before this module existed, and the
//! talker is still told what to speak. A typo in Settings costs nothing.
//!
//! Norwegian is why this exists. Bokmål and Nynorsk are separate labels in the
//! transcriber's language set. An unpinned session picks one per utterance, and
//! a short phrase is where it flips.

use sqlx::PgPool;

/// What one workspace's Locale language means to a call.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SpokenLanguage {
    /// ISO-639-1, when the name resolved to one. `None` leaves the transcriber
    /// guessing, exactly as it did before.
    pub code: Option<String>,
    /// The name as the user wrote it. This is what the talker is told to speak,
    /// so "Norwegian Bokmål" reaches the model as they typed it.
    pub name: String,
}

/// Every name we recognise, and the code it resolves to.
///
/// Each row is the transcriber's code, then the names that reach it. It covers
/// the languages the Locale dropdown offers, their native forms, and the
/// ISO-639-1 code typed directly.
///
/// `nb` resolves to `no`. Bokmål has its own ISO-639-1 code, and the
/// transcriber's set does not carry it: that set has Norwegian and Nynorsk, so
/// `no` is the one that means Bokmål.
const NAMES: &[(&str, &[&str])] = &[
    ("en", &["en", "english"]),
    (
        "no",
        &[
            "no",
            "nb",
            "norwegian",
            "norsk",
            "bokmål",
            "bokmal",
            "norwegian bokmål",
            "norwegian bokmal",
            "norsk bokmål",
        ],
    ),
    (
        "nn",
        &["nn", "nynorsk", "norwegian nynorsk", "norsk nynorsk"],
    ),
    ("sv", &["sv", "swedish", "svenska"]),
    ("da", &["da", "danish", "dansk"]),
    ("de", &["de", "german", "deutsch"]),
    ("fr", &["fr", "french", "français", "francais"]),
    ("es", &["es", "spanish", "español", "espanol", "castellano"]),
    ("it", &["it", "italian", "italiano"]),
    ("pt", &["pt", "portuguese", "português", "portugues"]),
    ("nl", &["nl", "dutch", "nederlands"]),
    ("pl", &["pl", "polish", "polski"]),
    ("fi", &["fi", "finnish", "suomi"]),
    ("ja", &["ja", "japanese", "日本語"]),
    ("zh", &["zh", "chinese", "中文"]),
    ("ko", &["ko", "korean", "한국어"]),
];

impl SpokenLanguage {
    /// Read one preference value as a language, or `None` when it says nothing.
    pub fn resolve(preference: &str) -> Option<Self> {
        let name = preference.trim();
        if name.is_empty() {
            return None;
        }
        let lowered = name.to_lowercase();
        let code = NAMES
            .iter()
            .find(|(_, names)| names.contains(&lowered.as_str()))
            .map(|(code, _)| (*code).to_string());
        Some(Self {
            code,
            name: name.to_string(),
        })
    }
}

/// This workspace's spoken language, or `None` when Locale is on Auto.
///
/// A read error reads as unset, for the reason `read_pref` gives: a session
/// that will not open over one unreachable row is worse than one that guesses
/// the language.
pub async fn for_workspace(pool: &PgPool) -> Option<SpokenLanguage> {
    super::read_pref(pool, "language")
        .await
        .as_deref()
        .and_then(SpokenLanguage::resolve)
}

#[cfg(test)]
mod tests {
    use super::*;

    /// The Locale dropdown itself, read at compile time. The same reach the
    /// engine already makes for the shared stylesheet in `api/sdk.rs`.
    const LOCALE_SECTION: &str =
        include_str!("../../../lucidos-app/src/components/settings/LocaleSection.tsx");

    fn resolved(preference: &str) -> SpokenLanguage {
        SpokenLanguage::resolve(preference).expect("a non-blank preference resolves")
    }

    /// The drift this guards is a language added to the dropdown and not here.
    /// It would offer the user a language that pins no code, which is the
    /// defect this module fixed, arriving through the other file.
    ///
    /// A `.tsx`-only diff does not compile this, so `/harden` Phase 4.5 carries
    /// a row pointing `LocaleSection.tsx` at `voice::language`.
    #[test]
    fn every_language_the_dropdown_offers_resolves_to_a_code() {
        let start = LOCALE_SECTION
            .find("const COMMON_LANGUAGES = [")
            .expect("the dropdown still declares COMMON_LANGUAGES");
        let list = &LOCALE_SECTION[start..];
        let end = list.find("];").expect("the list is still closed");
        let body = &list[..end];
        let offered: Vec<&str> = body.split('\'').skip(1).step_by(2).collect();

        // Count the entries independently of how they are quoted. Reading only
        // single-quoted ones would skip a `"Ukrainian",` added in double quotes
        // and still pass, which is the drift this exists to catch.
        let entries = body.matches(',').count();
        assert_eq!(
            offered.len(),
            entries,
            "read {} single-quoted names out of {} entries, so the list is \
             quoted some other way now",
            offered.len(),
            entries
        );
        assert!(
            offered.len() > 10,
            "read {:?}, which is not the dropdown's list",
            offered
        );
        for name in offered {
            assert!(
                resolved(name).code.is_some(),
                "the Locale dropdown offers '{}', which pins no transcriber code",
                name
            );
        }
    }

    /// The language is stated once, in `instructions_for`. A resident section
    /// reading it back would put one rule in two places. The block is what the
    /// talker KNOWS, not what it must do.
    #[test]
    fn no_resident_section_states_the_language() {
        use crate::test_support::source_scan::{read_production_source, src_root};

        let sections = read_production_source(&src_root().join("voice/sections.rs"));
        assert!(
            !sections.contains("\"language\""),
            "voice/sections.rs reads the language preference again, so the \
             resident block and the instructions can now disagree"
        );
    }

    #[test]
    fn a_dropdown_name_reaches_the_transcriber_as_a_code() {
        assert_eq!(resolved("Norwegian").code.as_deref(), Some("no"));
        assert_eq!(resolved("English").code.as_deref(), Some("en"));
        assert_eq!(resolved("Japanese").code.as_deref(), Some("ja"));
    }

    /// The whole reason this module exists. Bokmål and Nynorsk are separate
    /// labels, so each has to reach its own code rather than one covering both.
    #[test]
    fn bokmal_and_nynorsk_resolve_apart() {
        assert_eq!(resolved("Norwegian Bokmål").code.as_deref(), Some("no"));
        assert_eq!(resolved("nb").code.as_deref(), Some("no"));
        assert_eq!(resolved("Nynorsk").code.as_deref(), Some("nn"));
    }

    #[test]
    fn a_native_name_and_a_bare_code_both_resolve() {
        assert_eq!(resolved("norsk").code.as_deref(), Some("no"));
        assert_eq!(resolved("Deutsch").code.as_deref(), Some("de"));
        assert_eq!(resolved("sv").code.as_deref(), Some("sv"));
    }

    #[test]
    fn the_match_ignores_case_and_surrounding_space() {
        assert_eq!(resolved("  SWEDISH  ").code.as_deref(), Some("sv"));
        assert_eq!(resolved("  SWEDISH  ").name, "SWEDISH");
    }

    /// The talker is still told what to speak. Only the transcriber goes
    /// without, which is exactly its behaviour before this module.
    #[test]
    fn an_unknown_name_keeps_its_name_and_pins_no_code() {
        let language = resolved("Klingon");
        assert_eq!(language.code, None);
        assert_eq!(language.name, "Klingon");
    }

    #[test]
    fn a_blank_preference_is_auto() {
        assert_eq!(SpokenLanguage::resolve(""), None);
        assert_eq!(SpokenLanguage::resolve("   "), None);
    }

    /// A name reaching two codes would make the resolution order load-bearing,
    /// and the order here is alphabetical rather than considered.
    #[test]
    fn no_name_is_claimed_by_two_languages() {
        let mut seen: Vec<&str> = NAMES
            .iter()
            .flat_map(|(_, names)| *names)
            .copied()
            .collect();
        let count = seen.len();
        seen.sort_unstable();
        seen.dedup();
        assert_eq!(seen.len(), count, "two languages claim one name");
    }

    /// Every name is matched against a lowercased input, so an entry carrying a
    /// capital can never be reached.
    #[test]
    fn every_name_is_written_lowercase() {
        for (code, names) in NAMES {
            for name in *names {
                assert_eq!(
                    &name.to_lowercase(),
                    name,
                    "{} has an unreachable name",
                    code
                );
            }
        }
    }
}
