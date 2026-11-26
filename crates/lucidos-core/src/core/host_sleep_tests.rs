use super::*;

const QUARTER_HOUR: Duration = Duration::from_secs(15 * 60);

struct Timeline {
    origin: Instant,
}

impl Timeline {
    fn new() -> Self {
        Self {
            origin: Instant::now(),
        }
    }

    fn at(&self, wall: &str, awake_secs: u64) -> ClockReading {
        let wall = chrono::DateTime::parse_from_rfc3339(wall).expect("test time parses");
        ClockReading {
            wall: wall.into(),
            mono: self.origin + Duration::from_secs(awake_secs),
        }
    }
}

fn round(
    previous_end: Option<ClockReading>,
    start: ClockReading,
    end: ClockReading,
) -> RoundClocks {
    RoundClocks {
        previous_end,
        start,
        end,
        period: QUARTER_HOUR,
    }
}

#[test]
fn an_awake_round_on_time_did_not_sleep() {
    let t = Timeline::new();
    let previous_end = t.at("2026-10-05T10:00:20Z", 0);
    let start = t.at("2026-10-05T10:15:01Z", 881);
    let end = t.at("2026-10-05T10:15:19Z", 899);

    assert_eq!(slept(&round(Some(previous_end), start, end)), None);
}

#[test]
fn a_wall_clock_jump_mid_probe_is_a_sleep() {
    let t = Timeline::new();
    let start = t.at("2026-10-05T02:45:02Z", 0);
    let end = t.at("2026-10-05T02:59:40Z", 12);

    let slept = slept(&round(None, start, end)).expect("the round overlapped a sleep");
    assert_eq!(slept, Duration::from_secs(14 * 60 + 38 - 12));
}

#[test]
fn the_dark_wake_tick_ten_minutes_late_is_a_sleep() {
    let t = Timeline::new();
    let start = t.at("2026-10-05T05:25:04Z", 0);
    let end = t.at("2026-10-05T05:25:49Z", 45);

    let slept = slept(&round(None, start, end)).expect("a late tick is a sleep sign");
    assert_eq!(
        slept,
        Duration::from_secs(10 * 60 + 4),
        "the host was asleep at least from the planned 07:15 on"
    );
}

#[test]
fn a_round_just_before_sleep_follows_one_from_another_wake() {
    let t = Timeline::new();
    let previous_end = t.at("2026-10-05T02:30:40Z", 0);
    let start = t.at("2026-10-05T02:48:50Z", 40);
    let end = t.at("2026-10-05T02:49:26Z", 76);

    let slept = slept(&round(Some(previous_end), start, end)).expect("it slept between rounds");
    assert_eq!(slept, Duration::from_secs(18 * 60 + 10 - 40));
}

#[test]
fn a_sleep_between_rounds_shows_even_when_the_tick_is_on_time() {
    let t = Timeline::new();
    let previous_end = t.at("2026-10-05T03:00:25Z", 0);
    let start = t.at("2026-10-05T03:15:00Z", 30);
    let end = t.at("2026-10-05T03:15:20Z", 50);

    assert_eq!(
        slept(&round(Some(previous_end), start, end)),
        Some(Duration::from_secs(14 * 60 + 35 - 30))
    );
}

#[test]
fn a_late_tick_after_a_round_that_slept_counts_no_minutes_twice() {
    let t = Timeline::new();
    let previous_end = t.at("2026-10-05T10:20:00Z", 15);
    let start = t.at("2026-10-05T10:20:01Z", 16);
    let end = t.at("2026-10-05T10:20:21Z", 36);

    assert_eq!(
        slept(&round(Some(previous_end), start, end)),
        Some(Duration::ZERO)
    );
}

#[test]
fn clock_noise_is_not_a_sleep() {
    let t = Timeline::new();
    let start = t.at("2026-10-05T10:15:00Z", 0);
    let end = t.at("2026-10-05T10:15:25Z", 20);
    assert_eq!(slept(&round(None, start, end)), None);

    let previous_end = t.at("2026-10-05T10:17:20Z", 0);
    let start = t.at("2026-10-05T10:30:00Z", 900);
    let end = t.at("2026-10-05T10:30:10Z", 910);
    assert_eq!(slept(&round(Some(previous_end), start, end)), None);

    let start = t.at("2026-10-05T10:45:20Z", 0);
    let end = t.at("2026-10-05T10:45:30Z", 10);
    assert_eq!(slept(&round(None, start, end)), None);
}

#[test]
fn a_tick_is_planned_on_the_utc_quarter_hour() {
    let t = Timeline::new();
    let planned = t.at("2026-10-05T10:45:00Z", 0).wall;
    assert_eq!(tick_lateness(planned, QUARTER_HOUR), Duration::ZERO);
    let late = t.at("2026-10-05T10:46:01Z", 0).wall;
    assert_eq!(tick_lateness(late, QUARTER_HOUR), Duration::from_secs(61));
}
