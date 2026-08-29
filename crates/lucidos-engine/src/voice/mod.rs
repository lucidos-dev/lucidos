//! Voice: a rented talker holds the conversation, on an ordinary chat thread.
//!
//! Voice is a mode of a thread, never a kind of one (ADR 0148). The talker is
//! rented and tool-less, and the Lucidos Agent beside it is untouched (ADR
//! 0149). This module owns the seam both sit behind, and nothing above it names
//! a provider.
//!
//! The plan is `docs/plans/2026-08-29-a-voice-session-opens-behind-one-seam.md`.

pub mod build;
pub mod call;
pub mod provider;
pub mod realtime;
pub mod reasoner;
pub mod recovery;
pub mod registry;
pub mod resident;
pub mod sections;
pub mod wire;

#[cfg(test)]
pub mod mock;

#[cfg(test)]
#[path = "purity_tests.rs"]
mod purity_tests;

pub use provider::{AudioFormat, SessionOpening, VoiceEvent, VoiceProvider, VoiceSession};
pub use sections::{ResidentSection, SECTIONS};

/// What the talker is told it is, before it is told anything about the user.
///
/// The stable half of a session's prefix, so it is worth caching and worth
/// keeping free of anything per-session. Three rules, all from ADR 0149:
///
/// - It speaks as Lucidos, in the first person. The user meets one entity.
/// - It may stall truthfully, because work really is running on its behalf.
/// - It may not state a fact it did not receive. It holds no tools, so a
///   confident first-person claim is a fabrication rather than a paraphrase.
pub const TALKER_INSTRUCTIONS: &str = "\
You are Lucidos, speaking with the user out loud. Speak in the first person, as \
one assistant. Never mention that any other model or agent exists.

You are having a spoken conversation, so keep replies short and plain. Say \
numbers, dates and names the way a person would say them aloud.

You cannot look anything up, run anything, or change anything. Everything you \
know is in this conversation and in the context block you were opened with.

Never state a fact you were not given. If you do not have the answer, say so, \
and say that you are getting it. Work really is running for you, so it is \
honest to say you are checking. It is not honest to say you checked.";

#[cfg(test)]
mod tests {
    use super::mock::MockVoiceProvider;
    use super::*;
    use crate::engine::ApiUsage;

    fn opening() -> SessionOpening {
        SessionOpening {
            instructions: TALKER_INSTRUCTIONS.to_string(),
            resident_block: "[RESIDENT] the block".to_string(),
            voice: "marin".to_string(),
            audio: AudioFormat::default(),
        }
    }

    #[tokio::test]
    async fn the_resident_block_is_the_first_history_item() {
        let provider = MockVoiceProvider::new(vec![]);
        let log = provider.log();
        let mut session = provider.open(opening()).await.expect("open");
        session
            .append_context("[PROGRESS] still working")
            .await
            .expect("append");

        let log = log.lock().expect("log");
        assert_eq!(log.history[0], "[RESIDENT] the block");
        assert_eq!(log.history.len(), 2);
    }

    #[tokio::test]
    async fn every_append_lands_last_and_rewrites_nothing() {
        let provider = MockVoiceProvider::new(vec![]);
        let log = provider.log();
        let mut session = provider.open(opening()).await.expect("open");
        for note in ["first", "second", "third"] {
            session.append_context(note).await.expect("append");
        }

        let log = log.lock().expect("log");
        assert_eq!(
            log.history,
            vec!["[RESIDENT] the block", "first", "second", "third"]
        );
    }

    #[tokio::test]
    async fn a_session_replays_its_script_then_ends() {
        let usage = ApiUsage {
            input_tokens: 900,
            output_tokens: 40,
            cache_read_tokens: 800,
            cache_creation_tokens: 0,
        };
        let provider = MockVoiceProvider::ending_after(vec![
            VoiceEvent::UserTurnEnded {
                transcript: "what is on today".to_string(),
            },
            VoiceEvent::TalkerTurnEnded {
                transcript: "checking".to_string(),
                usage,
            },
        ]);
        let mut session = provider.open(opening()).await.expect("open");

        assert!(matches!(
            session.next().await,
            Some(VoiceEvent::UserTurnEnded { .. })
        ));
        assert!(matches!(
            session.next().await,
            Some(VoiceEvent::TalkerTurnEnded { .. })
        ));
        assert_eq!(session.next().await, None);
    }

    #[tokio::test]
    async fn caller_audio_is_forwarded_and_never_kept() {
        let provider = MockVoiceProvider::new(vec![]);
        let log = provider.log();
        let mut session = provider.open(opening()).await.expect("open");
        session.push_audio(&[0u8; 480]).await.expect("push");
        session.push_audio(&[0u8; 480]).await.expect("push");

        assert_eq!(log.lock().expect("log").audio_in_bytes, 960);
    }

    #[tokio::test]
    async fn a_provider_that_cannot_open_says_why() {
        let provider = MockVoiceProvider::refusing("no voice provider is configured");
        let error = provider.open(opening()).await.err().expect("should refuse");
        assert_eq!(error.to_string(), "no voice provider is configured");
    }

    #[test]
    fn the_talker_is_told_it_can_neither_act_nor_invent() {
        assert!(TALKER_INSTRUCTIONS.contains("cannot look anything up"));
        assert!(TALKER_INSTRUCTIONS.contains("Never state a fact you were not given"));
    }
}
