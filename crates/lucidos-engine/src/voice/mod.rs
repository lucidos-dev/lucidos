//! Voice: a rented talker holds the conversation, on an ordinary chat thread.
//!
//! Voice is a mode of a thread, never a kind of one (ADR 0148). The talker is
//! rented, and the Lucidos Agent beside it is untouched (ADR 0149). This module
//! owns the seam both sit behind, and nothing above it names a provider.
//!
//! **Talker and DOER are the two halves.** The doer holds every tool and the
//! talker holds three, so what splits them is capability rather than count.
//! Those three are the whole of the talker's reach: it asks for work, it
//! answers what is waiting on the user, and it hangs up (ADR 0170).
//!
//! The plan is `docs/plans/2026-08-29-a-voice-session-opens-behind-one-seam.md`.

pub mod build;
pub mod call;
pub mod decision;
pub mod doer;
pub mod language;
pub mod provider;
pub mod realtime;
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

use decision::DecisionChoice;
pub use language::SpokenLanguage;
pub use provider::{AudioFormat, SessionOpening, VoiceEvent, VoiceProvider, VoiceSession};
pub use sections::{ResidentSection, SECTIONS};

/// A preference's value when set to something non-blank.
///
/// A read error reads as unset. A session that will not open over one
/// unreachable preference row is worse than one that opens knowing less.
///
/// Private, so it reaches this module's own children and nothing else. Every
/// caller is a session-open path, and no other part of the engine reads a
/// preference this leniently.
async fn read_pref(pool: &sqlx::PgPool, key: &str) -> Option<String> {
    match crate::core::PreferenceStore::get(pool, key).await {
        Ok(Some(v)) if !v.trim().is_empty() => Some(v),
        Ok(_) => None,
        Err(e) => {
            log!(
                "[Voice] Could not read {}: {}. Treating it as unset",
                key,
                e
            );
            None
        }
    }
}

/// How long one thing the talker reads may be before it is cut.
///
/// A turn it recalls, and a choice on a question it puts to the caller. Both
/// are one utterance, and both come from somewhere with no length contract.
pub(super) const READ_ALOUD_CHARS: usize = 400;

/// Cut `text` to fit, on a char boundary, marking that it was cut.
///
/// Newlines go first: what comes back is one line, because the talker reads it
/// as one. `max` is compared against a char count and then applied as a byte
/// index, so multi-byte text is cut SHORTER than `max` characters. Safe in the
/// direction that matters, and never mid-character.
pub(super) fn clip(text: &str, max: usize) -> String {
    let flat = text.replace('\n', " ");
    if flat.chars().count() <= max {
        return flat;
    }
    let end = flat.floor_char_boundary(max);
    format!("{}…", &flat[..end])
}

/// The choices on an open decision, as lines the talker reads out.
///
/// Each line carries the id the engine issued for that choice, because handing
/// one back is the whole of how the talker settles anything. The id is for the
/// tool and never for the caller's ear: the two surfaces below say so, and so
/// does the tool's own description.
///
/// Never empty. A decision with no choice is one nothing can settle, and
/// `voice::decision` refuses to build one.
///
/// Shared by the two places a decision reaches the talker: the *resident
/// block* a call opens with, and the note handed over when one lands mid-call.
/// One wording, so the caller hears the same thing whichever route it took.
pub(super) fn choices_for(choices: &[DecisionChoice]) -> String {
    let mut out = String::from("The choices, with the id to hand back for each:\n");
    for choice in choices {
        match choice.description.as_deref() {
            Some(detail) => {
                out.push_str(&format!("- {} [{}]: {}\n", choice.label, choice.id, detail))
            }
            None => out.push_str(&format!("- {} [{}]\n", choice.label, choice.id)),
        }
    }
    out
}

/// What the talker is told it is, before it is told anything about the user.
///
/// The stable half of a session's prefix, so it is worth caching and worth
/// keeping free of anything per-session. Five rules:
///
/// - It speaks as Lucidos, in the first person. The user meets one entity.
/// - It does nothing itself. Its tools ask, answer and hang up. Calling one is
///   the only way anything gets looked up or done (ADR 0149, and ADR 0170,
///   which supersedes its tool-less clause).
/// - It may stall truthfully, because work really is running on its behalf.
/// - It may not state a fact it did not receive. A confident first-person
///   claim is a fabrication rather than a paraphrase.
/// - An answer it is handed is a source, not a script. What arrives was
///   written for a reader, and the caller is listening.
///
/// It is never told whether a turn is running, because it cannot see one. The
/// tool means the same thing either way, and the engine decides what that is.
pub const TALKER_INSTRUCTIONS: &str = "\
You are Lucidos, speaking with the user out loud. Speak in the first person, as \
one assistant. Never mention that any other model or agent exists.

You are having a spoken conversation, so keep replies short and plain. Say \
numbers, dates and names the way a person would say them aloud.

You look nothing up and do nothing yourself. What you know is in this \
conversation and in the context block you were opened with. For everything \
else you have tools, and calling one is the only way anything is found out or \
done.

Reach for them early. Ask for work the moment you hear something that block \
does not answer, and for anything the user wants done. Speak in the same turn: \
tell them you are on it, then stop and wait.

When something is waiting on the user, they can settle it by saying so. Put it \
to them out loud, and hand back the choice they pick.

Never state a fact you were not given. If you do not have the answer, say so, \
and say that you are getting it. Work really is running for you, so it is \
honest to say you are checking. It is not honest to say you checked.

When you are given an answer to pass on, say what it means. Never read it out. \
It was written to be read, and it can carry headings, tables, code and links, \
none of which can be spoken. The user has the full text in front of them, so \
your job is to tell them what it says.";

/// The talker's tools. Named here, so nothing above the seam can add a fourth.
///
/// Three, and each is on the harmless side of ADR 0149's line: one starts an
/// ordinary doer turn, one settles a card the caller can already see, and one
/// ends the call. None mutates the workspace, and none reaches a capability the
/// doer does not already gate (ADR 0170).
pub const DELEGATE_TOOL: &str = "delegate";
pub const ANSWER_TOOL: &str = "answer";
pub const HANGUP_TOOL: &str = "hang_up";

/// The one argument `delegate` takes: the talker's own words for what is
/// wanted.
pub const DELEGATE_REASON_ARG: &str = "reason";

/// The one argument `answer` takes: a choice id the ENGINE issued.
///
/// Never a label and never a spoken word. That is the whole reason no matching
/// is needed, and why the workspace's language cannot break it.
pub const ANSWER_CHOICE_ARG: &str = "choice";

/// What the talker is told the tool is for.
///
/// It biases hard toward calling, because nothing corrects a stale resident
/// block once the doer stops running on every turn. Under-calling is the
/// expensive mistake: it answers confidently from a snapshot. Over-calling
/// costs one turn nobody hears.
///
/// It names no state of the doer's, because the talker cannot see one.
pub const DELEGATE_TOOL_DESCRIPTION: &str = "\
Use this for anything the context block you were opened with cannot answer. \
Use it for anything the user wants done, changed, sent or found.

Call it even when you think you know. That block is a snapshot from the moment \
this conversation opened, and nothing updates it while you talk.

Call it again for every new thing the user asks, including one they ask while \
earlier work is still going. Nothing is queued up for you, so each call is \
what carries that request through.

Speak in the same turn you call it. Tell the user you are on it, then stop and \
wait. What comes back arrives later, as something for you to pass on.

Never say this tool's name out loud, and never suggest anything but you is \
involved.";

/// What the talker is told the answering tool is for.
///
/// Every choice it can pick was issued by the engine and read to it. So the
/// instruction is to hand one back, never to compose one: an id it invents
/// answers nothing, and the engine says so rather than guessing.
pub const ANSWER_TOOL_DESCRIPTION: &str = "\
Use this to answer something the user is being asked. What is open is in the \
context block, and each choice there carries an id.
Hand back the id of the choice they picked, exactly as it is written. Never \
read an id out loud, and never make one up.
When what they said fits none of the choices, use the one that sends their own \
words. That is also how they pick more than one.
Only call this once they have actually chosen. Somebody thinking out loud has \
not answered yet, so wait for them.
Never say this tool's name out loud, and never suggest anything but you is \
involved.";

/// What the talker is told the hangup tool is for.
///
/// Four rules, and the first is the load-bearing one: the caller's own words
/// trigger it. A talker that ends a call on its own judgment hangs up on
/// somebody who was thinking.
pub const HANGUP_TOOL_DESCRIPTION: &str = "\
Use this when the user says the conversation is over: goodbye, that is all, \
thanks that is everything. It ends the call.
Say goodbye first, in the same turn, and call this after. It closes the line, \
so anything you say afterwards is something they never hear.
Only their words end a call. Silence does not, because somebody who stops \
talking is thinking. Never call this because you have run out of things to say.
Work in flight keeps going. This ends the call and never the work.";

/// What this workspace's talker is told, language included.
///
/// The language belongs here rather than in the resident block. That block is
/// what the talker KNOWS, and which language to speak is a rule it follows.
/// Saying it in both places is how the two come to disagree.
///
/// A workspace-level fact, so the prefix a session opens with is still stable
/// across that workspace's calls and still worth caching. Nothing per-session
/// may follow it in.
pub fn instructions_for(language: Option<&SpokenLanguage>) -> String {
    match language {
        None => TALKER_INSTRUCTIONS.to_string(),
        Some(language) => format!(
            "{}\n\nSpeak {}. Use it even when the caller uses another language.",
            TALKER_INSTRUCTIONS, language.name
        ),
    }
}

#[cfg(test)]
mod tests {
    use super::decision::OpenDecision;
    use super::mock::MockVoiceProvider;
    use super::*;
    use crate::engine::thread_events::QuestionOption;
    use crate::engine::ApiUsage;

    fn opening() -> SessionOpening {
        SessionOpening {
            instructions: TALKER_INSTRUCTIONS.to_string(),
            resident_block: "[RESIDENT] the block".to_string(),
            voice: "marin".to_string(),
            transcriber: "gpt-4o-mini-transcribe".to_string(),
            audio: AudioFormat::default(),
            language: None,
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
            modality: None,
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
    fn a_clipped_line_never_splits_a_character() {
        let text = "é".repeat(50);
        let clipped = clip(&text, 10);
        assert!(clipped.starts_with('é'));
        assert!(clipped.ends_with('…'));
    }

    #[test]
    fn a_clipped_line_loses_its_newlines_so_one_thing_is_one_line() {
        assert_eq!(clip("a\nb\nc", 100), "a b c");
    }

    fn options() -> Vec<QuestionOption> {
        vec![
            QuestionOption {
                id: "opt-0".to_string(),
                label: "Run the tail now".to_string(),
                description: Some("Chunks 25-33".to_string()),
            },
            QuestionOption {
                id: "opt-1".to_string(),
                label: "Leave it".to_string(),
                description: None,
            },
        ]
    }

    /// One wording, so a decision reads the same whether it reached the talker
    /// in the opening block or as a note mid-call.
    #[test]
    fn the_choices_carry_a_description_when_there_is_one() {
        let decision = OpenDecision::question("toolu_q0", "Now?", &options(), false);
        let block = choices_for(&decision.choices);
        assert!(
            block.contains("- Run the tail now [question:toolu_q0#opt0]: Chunks 25-33\n"),
            "{}",
            block
        );
        assert!(
            block.contains("- Leave it [question:toolu_q0#opt1]\n"),
            "{}",
            block
        );
    }

    /// Handing an id back is the whole of how the talker settles anything, so
    /// every line carries one.
    #[test]
    fn every_choice_reads_out_with_the_id_that_settles_it() {
        let decision = OpenDecision::question("toolu_q0", "Now?", &options(), false);
        let block = choices_for(&decision.choices);
        assert!(block.contains("id to hand back"), "{}", block);
        for choice in &decision.choices {
            assert!(
                block.contains(&choice.id),
                "{} missing from {}",
                choice.id,
                block
            );
        }
    }

    #[test]
    fn the_talker_is_told_it_can_neither_act_nor_invent() {
        assert!(TALKER_INSTRUCTIONS.contains("do nothing yourself"));
        assert!(TALKER_INSTRUCTIONS.contains("Never state a fact you were not given"));
    }

    /// It is told a tool call is the only way anything happens, and to reach
    /// for one early. Under-calling is the expensive mistake: nothing corrects
    /// a stale resident block once the doer stops running every turn.
    #[test]
    fn the_talker_is_told_a_tool_is_the_only_way_anything_happens() {
        assert!(TALKER_INSTRUCTIONS.contains("you have tools"));
        assert!(TALKER_INSTRUCTIONS.contains("the only way anything is found out"));
        assert!(TALKER_INSTRUCTIONS.contains("Reach for them early"));
    }

    /// It is told the caller can settle what is waiting by saying so. The two
    /// surfaces that read a card out say the same, and the code now keeps it.
    #[test]
    fn the_talker_is_told_the_caller_can_settle_things_out_loud() {
        assert!(TALKER_INSTRUCTIONS.contains("they can settle it by saying so"));
        assert!(TALKER_INSTRUCTIONS.contains("hand back the choice they pick"));
    }

    /// Three tools, each named once. A fourth cannot arrive from above the
    /// seam, so this is the whole of the talker's reach (ADR 0170).
    #[test]
    fn the_talker_holds_three_tools_and_each_says_what_it_is_for() {
        let named = [
            (DELEGATE_TOOL, DELEGATE_TOOL_DESCRIPTION),
            (ANSWER_TOOL, ANSWER_TOOL_DESCRIPTION),
            (HANGUP_TOOL, HANGUP_TOOL_DESCRIPTION),
        ];
        let mut names: Vec<&str> = named.iter().map(|(name, _)| *name).collect();
        names.sort_unstable();
        names.dedup();
        assert_eq!(names.len(), 3, "two tools share a name");
        for (name, description) in named {
            assert!(!name.is_empty());
            assert!(description.len() > 100, "{} is barely described", name);
        }
    }

    /// The answering tool hands back an id, never a word the caller said. That
    /// is the whole reason no matching is needed, in any language.
    #[test]
    fn the_answering_tool_is_told_to_hand_back_an_id_and_never_invent_one() {
        assert!(ANSWER_TOOL_DESCRIPTION.contains("Hand back the id"));
        assert!(ANSWER_TOOL_DESCRIPTION.contains("Never read an id out loud"));
        assert!(ANSWER_TOOL_DESCRIPTION.contains("never make one up"));
    }

    /// The caller's intent ends a call, never the talker's judgment, and never
    /// silence. Saying goodbye cancels nothing that is running.
    #[test]
    fn the_hangup_tool_is_triggered_by_the_caller_and_ends_no_work() {
        assert!(HANGUP_TOOL_DESCRIPTION.contains("Only their words end a call"));
        assert!(HANGUP_TOOL_DESCRIPTION.contains("Silence does not"));
        assert!(HANGUP_TOOL_DESCRIPTION.contains("never the work"));
        assert!(HANGUP_TOOL_DESCRIPTION.contains("Say goodbye first"));
    }

    /// The talker cannot see whether a turn is running, so nothing it reads
    /// may ask it to.
    ///
    /// The shortlist is phrases that could only mean the doer's state. Bare
    /// "running" is not among them: the honest-stall paragraph says work IS
    /// running for the caller, which is a promise rather than a condition.
    #[test]
    fn the_talker_is_never_asked_about_the_doers_state() {
        let assembled = instructions_for(SpokenLanguage::resolve("Norwegian").as_ref());
        let whole = format!(
            "{} {} {} {}",
            assembled, DELEGATE_TOOL_DESCRIPTION, ANSWER_TOOL_DESCRIPTION, HANGUP_TOOL_DESCRIPTION
        )
        .to_lowercase();
        for phrase in [
            "idle",
            "busy",
            "already working",
            "already running",
            "still running",
            "one at a time",
            "wait until",
        ] {
            assert!(
                !whole.contains(phrase),
                "the talker was asked to read the doer's state: {:?}",
                phrase
            );
        }
    }

    /// The talker reads a name, the transcriber reads a code. A name nobody can
    /// map still reaches the talker, which is the half a code cannot carry.
    #[test]
    fn the_talker_is_told_which_language_to_speak() {
        let known = SpokenLanguage::resolve("Norwegian Bokmål");
        let spoken = instructions_for(known.as_ref());
        assert!(spoken.contains("Speak Norwegian Bokmål."), "{}", spoken);
        assert!(spoken.starts_with(TALKER_INSTRUCTIONS));

        let unmapped = SpokenLanguage::resolve("Klingon");
        assert!(instructions_for(unmapped.as_ref()).contains("Speak Klingon."));
    }

    /// Auto leaves the prefix exactly as it was, so a workspace that never set
    /// a language opens the session it opened before.
    #[test]
    fn no_language_leaves_the_instructions_untouched() {
        assert_eq!(instructions_for(None), TALKER_INSTRUCTIONS);
    }

    /// A workspace-level fact, so the cached prefix is stable across its calls.
    /// Anything per-session added here would end that.
    #[test]
    fn two_calls_on_one_workspace_open_with_the_same_prefix() {
        let language = SpokenLanguage::resolve("Norwegian");
        assert_eq!(
            instructions_for(language.as_ref()),
            instructions_for(language.as_ref())
        );
    }

    /// The rename left no half: one concept keeps one word
    /// (`.claude/rules/glossary.md`).
    ///
    /// The needle is assembled from pieces, so this scan cannot find itself.
    /// Scoped to shipping sources and the glossary. A landed plan keeps the
    /// word it was written with, being its own record. ADR 0149 keeps it once
    /// more, as the name of the published pattern.
    #[test]
    fn nothing_shipping_calls_the_doer_by_its_old_name() {
        let old = concat!("reas", "oner");
        let repo = std::path::Path::new(env!("CARGO_MANIFEST_DIR"))
            .ancestors()
            .nth(2)
            .expect("the repo root above crates/<name>")
            .to_path_buf();

        let mut offenders = Vec::new();
        let mut scanned = 0usize;
        let mut stack = vec![repo.join("crates")];
        while let Some(dir) = stack.pop() {
            for entry in std::fs::read_dir(&dir).expect("read_dir").flatten() {
                let path = entry.path();
                if path.is_dir() {
                    let name = entry.file_name().to_string_lossy().to_string();
                    if !matches!(name.as_str(), "node_modules" | "target" | "dist") {
                        stack.push(path);
                    }
                    continue;
                }
                let extension = path.extension().and_then(|e| e.to_str());
                if !matches!(extension, Some("rs" | "ts" | "tsx")) {
                    continue;
                }
                scanned += 1;
                let text = std::fs::read_to_string(&path).unwrap_or_default();
                if text.to_lowercase().contains(old) {
                    offenders.push(path.display().to_string());
                }
            }
        }

        let glossary = repo.join("docs/glossary.md");
        let text = std::fs::read_to_string(&glossary).expect("read the dev glossary");
        if text.to_lowercase().contains(old) {
            offenders.push(glossary.display().to_string());
        }

        // A moved directory would otherwise make this pass by reading nothing.
        assert!(scanned > 100, "the scan found only {} sources", scanned);
        assert!(
            offenders.is_empty(),
            "the tool-holding half of the voice pair is the doer, everywhere: {:?}",
            offenders
        );
    }
}
