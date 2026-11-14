//! Logs the Live talker's output timings, for one question: can a talker turn
//! end on the provider's own spoken timeline instead of on arrival cadence?
//!
//! That needs two facts no stored event holds. Do worded output deltas carry
//! `end_ms`, and do their arrival holes sit over abutting timestamps? And does
//! the talker's audio ever run dry, which the caller would hear as a pause?
//!
//! Temporary: `docs/temporary-measures.md` § Live output timing log.

use std::time::Duration;

use serde_json::Value;
use tokio::time::Instant;

use super::super::provider::AudioFormat;

/// An arrival hole between worded deltas worth logging. Shorter than
/// `TALKER_IDLE`, so every hole that ends a talker turn shows.
const WORD_HOLE: Duration = Duration::from_millis(500);

/// How long the talker's audio may run dry before it is logged.
const AUDIO_DRY: Duration = Duration::from_millis(150);

/// What the log remembers between frames.
#[derive(Default)]
pub(super) struct TimingLog {
    keys_logged: bool,
    last_words: Option<WordedDelta>,
    /// When the audio received so far finishes playing, at real time.
    audio_runs_out: Option<Instant>,
}

#[derive(Clone, Copy)]
struct WordedDelta {
    arrived: Instant,
    start_ms: Option<i64>,
    end_ms: Option<i64>,
}

impl WordedDelta {
    fn span(self) -> String {
        format!("{:?} to {:?} ms", self.start_ms, self.end_ms)
    }
}

impl TimingLog {
    /// Log a worded output transcript delta, and return the lines logged.
    pub(super) fn worded_delta(&mut self, value: &Value, now: Instant) -> Vec<String> {
        let delta = WordedDelta {
            arrived: now,
            start_ms: value.get("start_ms").and_then(Value::as_i64),
            end_ms: value.get("end_ms").and_then(Value::as_i64),
        };
        let mut lines = vec![];
        if !std::mem::replace(&mut self.keys_logged, true) {
            let keys: Vec<&str> = value
                .as_object()
                .map(|frame| frame.keys().map(String::as_str).collect())
                .unwrap_or_default();
            lines.push(format!(
                "[Voice] Timing: the first worded output delta has keys {:?}, {}",
                keys,
                delta.span()
            ));
        }
        if let Some(last) = self.last_words.replace(delta) {
            let hole = now.saturating_duration_since(last.arrived);
            if hole > WORD_HOLE {
                lines.push(format!(
                    "[Voice] Timing: worded output deltas arrived {} ms apart, \
                     the earlier at {}, the later at {}",
                    hole.as_millis(),
                    last.span(),
                    delta.span()
                ));
            }
        }
        log_all(&lines);
        lines
    }

    /// Log an output audio delta of `bytes` that ran dry, and return the lines.
    ///
    /// Each chunk extends when the audio received so far finishes playing. A
    /// chunk arriving after that point means the caller heard nothing between.
    pub(super) fn audio(&mut self, bytes: usize, now: Instant) -> Vec<String> {
        let mut lines = vec![];
        let starts = match self.audio_runs_out {
            Some(out) if now > out => {
                let dry = now - out;
                if dry > AUDIO_DRY {
                    lines.push(format!(
                        "[Voice] Timing: the talker's audio ran dry for {} ms",
                        dry.as_millis()
                    ));
                }
                now
            }
            Some(out) => out,
            None => now,
        };
        self.audio_runs_out = Some(starts + playing_time(bytes));
        log_all(&lines);
        lines
    }
}

/// How long `bytes` of PCM16 in the call's format take to play.
fn playing_time(bytes: usize) -> Duration {
    let format = AudioFormat::default();
    let bytes_per_sec = u64::from(format.sample_rate_hz) * u64::from(format.channels) * 2;
    Duration::from_micros(bytes as u64 * 1_000_000 / bytes_per_sec)
}

fn log_all(lines: &[String]) {
    for line in lines {
        log!("{}", line);
    }
}

#[cfg(test)]
mod tests {
    use serde_json::json;

    use super::*;

    fn said(start_ms: i64, end_ms: i64) -> Value {
        json!({
            "type": "session.output_transcript.delta",
            "delta": "word",
            "start_ms": start_ms,
            "end_ms": end_ms,
        })
    }

    /// One second of the call's audio.
    const SECOND: usize = 48_000;

    #[test]
    fn the_first_worded_delta_logs_its_keys_once() {
        let mut log = TimingLog::default();
        let at = Instant::now();
        let first = log.worded_delta(&said(0, 200), at);
        assert_eq!(first.len(), 1);
        assert!(first[0].contains("\"end_ms\""), "{}", first[0]);
        assert!(first[0].contains("Some(0) to Some(200) ms"), "{}", first[0]);
        assert!(log
            .worded_delta(&said(200, 400), at + Duration::from_millis(100))
            .is_empty());
    }

    #[test]
    fn a_hole_in_arrival_logs_both_spans() {
        let mut log = TimingLog::default();
        let at = Instant::now();
        log.worded_delta(&said(0, 1_000), at);
        let lines = log.worded_delta(&said(1_000, 1_400), at + Duration::from_millis(800));
        assert_eq!(lines.len(), 1);
        assert!(lines[0].contains("800 ms apart"), "{}", lines[0]);
        assert!(
            lines[0].contains("Some(0) to Some(1000) ms"),
            "{}",
            lines[0]
        );
        assert!(
            lines[0].contains("Some(1000) to Some(1400) ms"),
            "{}",
            lines[0]
        );
    }

    #[test]
    fn audio_that_arrives_ahead_of_playback_never_runs_dry() {
        let mut log = TimingLog::default();
        let at = Instant::now();
        assert!(log.audio(SECOND, at).is_empty());
        assert!(log
            .audio(SECOND, at + Duration::from_millis(900))
            .is_empty());
        assert!(log
            .audio(SECOND, at + Duration::from_millis(2_100))
            .is_empty());
    }

    #[test]
    fn audio_that_arrives_after_playback_ran_out_is_logged() {
        let mut log = TimingLog::default();
        let at = Instant::now();
        log.audio(SECOND / 2, at);
        let lines = log.audio(SECOND, at + Duration::from_millis(900));
        assert_eq!(
            lines,
            vec!["[Voice] Timing: the talker's audio ran dry for 400 ms".to_string()]
        );
    }
}
