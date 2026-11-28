pub(crate) const FULL_CAPTURE_ENV: &str = "LUCIDOS_EVAL_FULL_CAPTURE";

pub(crate) fn full_bodies() -> bool {
    static ENABLED: std::sync::OnceLock<bool> = std::sync::OnceLock::new();
    *ENABLED.get_or_init(|| enabled_value(std::env::var(FULL_CAPTURE_ENV).ok().as_deref()))
}

fn enabled_value(value: Option<&str>) -> bool {
    value.is_some_and(crate::core::prefs::env_switch_is_on)
}

pub fn body_cap(normal: usize) -> Option<usize> {
    match full_bodies() {
        true => None,
        false => Some(normal),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn only_a_truthy_value_lifts_the_caps() {
        for &on in crate::core::prefs::FLAG_ON_VALUES {
            assert!(enabled_value(Some(on)), "{on} should enable it");
        }
        for off in [None, Some(""), Some("0"), Some("false"), Some("maybe")] {
            assert!(!enabled_value(off), "{off:?} should leave it off");
        }
    }

    #[test]
    fn a_cap_is_the_callers_number_or_nothing() {
        assert_eq!(body_cap(8_000).is_some(), !full_bodies());
        if !full_bodies() {
            assert_eq!(body_cap(8_000), Some(8_000));
        }
    }
}
