//! Cron expression and timezone parsing safety net.
//!
//! These tests guard the parsing primitives the scheduler relies on
//! (`parse_standard_cron` + `chrono_tz`) so changes to the upstream crates
//! or to our wrapper can never silently break trigger scheduling.

#[cfg(test)]
mod tests {
    #[test]
    fn valid_cron_expressions_parse() {
        use crate::engine::tools::scheduler::parse_standard_cron;
        // These are the standard 6-field cron expressions used throughout Lucidos.
        let valid = [
            "0 0 8 * * *",    // daily at 08:00
            "0 */30 * * * *", // every 30 minutes
            "0 0 22 * * 1-5", // weekdays at 22:00
            "0 0 0 1 * *",    // first of every month
            "0 0 8,20 * * *", // 08:00 and 20:00
        ];
        for expr in &valid {
            let result = parse_standard_cron(expr);
            assert!(
                result.is_ok(),
                "Expected valid cron expression '{}' to parse, got: {:?}",
                expr,
                result.err()
            );
        }
    }

    #[test]
    fn invalid_cron_expression_fails() {
        use crate::engine::tools::scheduler::parse_standard_cron;
        let invalid = [
            "",             // empty
            "* * *",        // too few fields
            "0 0 25 * * *", // hour 25 doesn't exist
            "not a cron",   // nonsense
            "0 0 8 * *",    // 5 fields (missing seconds)
        ];
        for expr in &invalid {
            let result = parse_standard_cron(expr);
            assert!(
                result.is_err(),
                "Expected invalid cron expression '{}' to fail, but it parsed successfully",
                expr
            );
        }
    }

    #[test]
    fn valid_timezone_parses() {
        let valid = [
            "UTC",
            "Europe/Oslo",
            "America/New_York",
            "Asia/Tokyo",
            "US/Pacific",
        ];
        for tz in &valid {
            let result = tz.parse::<chrono_tz::Tz>();
            assert!(
                result.is_ok(),
                "Expected valid timezone '{}' to parse, got: {:?}",
                tz,
                result.err()
            );
        }
    }

    #[test]
    fn invalid_timezone_fails() {
        let invalid = [
            "",
            "Not/A/Timezone",
            "Oslo",
            "GMT+2", // chrono_tz doesn't accept offset-only
        ];
        for tz in &invalid {
            let result = tz.parse::<chrono_tz::Tz>();
            assert!(
                result.is_err(),
                "Expected invalid timezone '{}' to fail, but it parsed: {:?}",
                tz,
                result.ok()
            );
        }
    }

    #[test]
    fn cron_next_occurrence_is_in_the_future() {
        // Verify that parsed cron schedules produce future occurrences.
        // This is the core of run_task_loop's scheduling — if next() returns
        // None or a past time, the scheduler breaks.
        let schedule = crate::engine::tools::scheduler::parse_standard_cron("0 0 8 * * *").unwrap();
        let now = chrono::Utc::now();
        let next = schedule.upcoming(chrono::Utc).next();
        assert!(
            next.is_some(),
            "cron schedule must produce a next occurrence"
        );
        assert!(next.unwrap() > now, "next occurrence must be in the future");
    }
}
