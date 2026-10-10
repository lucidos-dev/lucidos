//! Kebab-case slugification, shared by every surface that turns a human-facing
//! name into a machine-safe identifier: trigger slugs
//! (`triggers::slugify_trigger_name_with_fallback`) and coding-agent branch
//! names (`engine::git_ops::branch_name`).
//!
//! The output charset is deliberately narrow, `[a-z0-9-]` with no leading or
//! trailing dash, because both consumers feed it into places where a stray
//! character is a hard failure: a trigger slug becomes a directory name, and a
//! branch name has to survive `git check-ref-format`. Lowercase-only is
//! load-bearing for the branch case: git refs are case-sensitive, but loose refs
//! are files, so on a case-insensitive filesystem (macOS default) `Foo` and
//! `foo` would collide.

/// Convert a human-facing name to a stable kebab-case slug.
///
/// - NFKD-normalize then strip combining marks ("Café" becomes "cafe")
/// - Lowercase ASCII alphanumerics; collapse other runs to `-`
/// - Trim leading/trailing dashes
///
/// Returns `""` when nothing survives (e.g. `"!!!"` or an emoji-only name).
/// Callers must supply their own fallback: see
/// `triggers::slugify_trigger_name_with_fallback` and
/// `engine::git_ops::branch_name::branch_slug`.
pub fn slugify_kebab(name: &str) -> String {
    use unicode_normalization::UnicodeNormalization;
    let normalized: String = name
        .nfkd()
        .filter(|c| !unicode_normalization::char::is_combining_mark(*c))
        .collect();
    let mut out = String::with_capacity(normalized.len());
    let mut last_dash = true;
    for c in normalized.chars() {
        if c.is_ascii_alphanumeric() {
            out.push(c.to_ascii_lowercase());
            last_dash = false;
        } else if !last_dash {
            out.push('-');
            last_dash = true;
        }
    }
    out.trim_matches('-').to_string()
}

/// Truncate a slug to at most `max` characters, preferring to cut at a dash so
/// the result ends on a whole word. Falls back to a hard cut when the only dash
/// is too early to keep anything (a single very long word). Re-trims dashes, so
/// the result still satisfies the charset contract above.
///
/// Every cut goes through `floor_char_boundary`, per `.claude/rules/rust.md`.
/// [`slugify_kebab`] emits ASCII, so in practice the boundary is always exact,
/// but this is a `pub` helper and that safety would rest entirely on caller
/// discipline otherwise: one caller passing a raw name panics on a multi-byte
/// char instead of returning a shorter slug.
pub fn truncate_slug(slug: &str, max: usize) -> String {
    if slug.len() <= max {
        return slug.to_string();
    }
    let budget = slug.floor_char_boundary(max);
    // When the character just past the budget is a dash, the budget already
    // lands on a whole-word boundary and nothing has to be given back.
    let cut = if slug[budget..].starts_with('-') {
        budget
    } else {
        // Otherwise back up to the last dash, but only if that leaves at least
        // half the budget; a name with no early dash is one long word and a
        // hard cut is the honest answer.
        match slug[..budget].rfind('-') {
            Some(i) if i >= budget / 2 => i,
            _ => budget,
        }
    };
    slug[..cut].trim_matches('-').to_string()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn slug_kebab_strips_unicode_and_lowercases() {
        assert_eq!(
            slugify_kebab("Nightly Build → Harden"),
            "nightly-build-harden"
        );
        assert_eq!(slugify_kebab("Café Morning"), "cafe-morning");
        assert_eq!(slugify_kebab("My  Trigger!  v2"), "my-trigger-v2");
    }

    #[test]
    fn slug_kebab_is_empty_when_nothing_survives() {
        assert_eq!(slugify_kebab("!!!"), "");
        assert_eq!(slugify_kebab(""), "");
        assert_eq!(slugify_kebab("日本語"), "");
    }

    #[test]
    fn truncate_prefers_a_dash_boundary() {
        assert_eq!(
            truncate_slug("fix-the-auth-timeout-bug", 12),
            "fix-the-auth"
        );
        // No truncation needed.
        assert_eq!(truncate_slug("short", 12), "short");
    }

    #[test]
    fn truncate_hard_cuts_a_single_long_word() {
        let long = "a".repeat(30);
        assert_eq!(truncate_slug(&long, 10), "a".repeat(10));
    }

    #[test]
    fn truncate_never_leaves_a_trailing_dash() {
        // Cutting exactly on the dash would otherwise leave "fix-".
        assert_eq!(truncate_slug("fix-averyverylongword", 4), "fix");
    }

    /// Callers pass `slugify_kebab` output (ASCII) today, but the cut must not
    /// panic if one ever passes a raw name.
    #[test]
    fn truncate_does_not_split_a_multibyte_char() {
        // "é" is two bytes; a budget of 3 lands inside the second one.
        assert_eq!(truncate_slug("aéb", 3), "aé");
        assert_eq!(truncate_slug("日本語", 4), "日");
    }
}
