use sqlx::PgPool;

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SpokenLanguage {
    pub code: Option<String>,
    pub name: String,
}

/// INVARIANT: Every name we recognise, and the code it resolves to.
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

pub async fn for_workspace(pool: &PgPool) -> Option<SpokenLanguage> {
    crate::core::prefs::LANGUAGE
        .read(pool)
        .await
        .as_deref()
        .and_then(SpokenLanguage::resolve)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn resolved(preference: &str) -> SpokenLanguage {
        SpokenLanguage::resolve(preference).expect("a non-blank preference resolves")
    }

    #[test]
    fn every_language_the_dropdown_offers_resolves_to_a_code() {
        const LOCALE_SECTION: &str =
            include_str!("../../../lucidos-app/src/components/settings/LocaleSection.tsx");
        let start = LOCALE_SECTION
            .find("const COMMON_LANGUAGES = [")
            .expect("the dropdown still declares COMMON_LANGUAGES");
        let list = &LOCALE_SECTION[start..];
        let end = list.find("];").expect("the list is still closed");
        let body = &list[..end];
        let offered: Vec<&str> = body.split('\'').skip(1).step_by(2).collect();

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

    #[test]
    fn a_dropdown_name_reaches_the_transcriber_as_a_code() {
        assert_eq!(resolved("Norwegian").code.as_deref(), Some("no"));
        assert_eq!(resolved("English").code.as_deref(), Some("en"));
        assert_eq!(resolved("Japanese").code.as_deref(), Some("ja"));
    }

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

    mod guard_tests {
        #[test]
        fn no_resident_section_states_the_language() {
            use crate::test_support::source_scan::{engine_source, read_production_source};

            let sections = read_production_source(&engine_source("voice/sections.rs"));
            assert!(
                !sections.contains("\"language\""),
                "voice/sections.rs reads the language preference again, so the \
             resident block and the instructions can now disagree"
            );
        }
    }
}
