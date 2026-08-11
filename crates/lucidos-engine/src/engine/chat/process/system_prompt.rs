//! Chat-agent system-prompt assembly. Builds the full per-turn system prompt
//! (identity, timezone, the large static base prompt, apps/intents/knowhow
//! listings, images section, trigger framing) plus the mandatory-setup
//! preference scan. Split out of `process_message_with_steps_internal`. The
//! `ASK_USER_QUESTION_RULE` prompt fragment lives here and is re-exported from
//! the parent for the prompt-content tests.

use crate::core::PreferenceStore;
use crate::engine::LucidosEngine;
use chrono::Utc;
use std::path::Path;

use super::super::process_helpers::{
    build_system_knowhow_section, build_trigger_knowhow_section, TriggerContext,
    APPLY_VERIFY_DEV_ADDENDUM, APPLY_VERIFY_RULE, ENGINE_RESTART_RULE, LOOK_BEFORE_ASSESSING_RULE,
};

/// What this install lets a coding agent edit, when the engine WAS launched
/// from a Lucidos source checkout. Paired with [`NO_LUCIDOS_SOURCE_SECTION`];
/// exactly one of the two is in every prompt (see [`coding_surface_section`]).
const LUCIDOS_SOURCE_SECTION: &str = "\n\nWHAT A CODING AGENT CAN EDIT ON THIS INSTALL:\n\
     This engine is running from a Lucidos SOURCE CHECKOUT, so you can edit the \
     Lucidos platform's own code: call `run_coding_agent` with `folder` omitted \
     and the session runs against the source tree. A change to Rust or backend \
     files needs the user to Apply and then trigger the rebuild and restart \
     before it is live. You can also edit an installed app \
     (`folder=\"data/apps/<id>\"`) or a repository registered via \
     `manage_repositories`.";

/// What this install lets a coding agent edit, when there is NO Lucidos source
/// checkout (a packaged `.app` / headless install).
///
/// This is the fix for the reported failure: with nothing in the prompt saying
/// otherwise, the agent read the `run_coding_agent` description ("omit `folder`
/// to edit Lucidos itself"), asserted it had read `crates/lucidos-engine/…`,
/// spawned a session, and told the user to Apply and rebuild — on an install
/// with no source tree at all. The engine now refuses that spawn, but the model
/// must be TOLD, not just blocked, or it narrates the capability first and
/// discovers the refusal after the user has already been misled.
const NO_LUCIDOS_SOURCE_SECTION: &str = "\n\nWHAT A CODING AGENT CAN EDIT ON THIS INSTALL:\n\
     This engine was NOT launched from a Lucidos source checkout: this install \
     ships the binary only. So you CANNOT edit Lucidos itself, and a \
     `run_coding_agent` call with `folder` omitted and no `workspace`, i.e. \
     aimed at THIS install, is REFUSED.\n\
     - NEVER say or imply you have read, inspected, or can change Lucidos's own \
       source. Reason from observed behaviour and documentation, and say that is \
       what you are doing.\n\
     - NEVER tell the user to Apply, rebuild, or restart for a change to Lucidos \
       itself: this install updates through the app updater, not a rebuild.\n\
     - When they ask for one, say directly that it cannot be done here and offer \
       the cross-workspace route below. Never spawn a local coding agent to try \
       anyway.\n\
     What DOES work: an installed app with \
     `run_coding_agent(folder=\"data/apps/<id>\")`, and a repository registered \
     via `manage_repositories` with `run_coding_agent(folder=<repo name>)`.\n\
     CROSS-WORKSPACE IS STILL OPEN: the refusal is about THIS install, not about \
     you. If another workspace's engine DOES run from a Lucidos source checkout, \
     route platform work there with `run_coding_agent(workspace=\"<name>\", \
     relation=\"top\")` and `folder` omitted; the target engine applies its own \
     source check. Offer it when the user names such a workspace or you know of \
     one, never on a guess.";

/// Pick the coding-surface prompt section for this install.
///
/// The single divergence point between the two system-prompt variants — one
/// that says the agent can edit the source it is running on, one that says it
/// cannot. Pure over the flag so both variants are unit-testable without a
/// packaged binary; the caller supplies
/// [`crate::paths::has_lucidos_source`], which is the same signal behind the
/// `/health` `packaged` flag (so the compose picker and the chat agent can
/// never disagree about whether "Lucidos source" exists).
///
/// Constant per process, so splicing it does not disturb the provider
/// prompt-cache prefix the surrounding sections rely on.
pub(crate) fn coding_surface_section(has_lucidos_source: bool) -> &'static str {
    if has_lucidos_source {
        LUCIDOS_SOURCE_SECTION
    } else {
        NO_LUCIDOS_SOURCE_SECTION
    }
}

/// Nudges the Lucidos chat agent to use the `ask_user_question` tool for any
/// choice-shaped question (yes/no, A vs B, pick-from-list, "what next?"
/// follow-up menu) instead of writing the options as plaintext markdown. The
/// UI renders tool-driven options as clickable buttons; plaintext options
/// force the user to type. Carves the rule out of ACTION FIRST so the LLM
/// stops conflating "don't re-ask what the user already told you" with
/// "never offer next-step alternatives." Mirrors the CC-side
/// `ASK_USER_QUESTION_RULE` in `agent_session::prompts` (same spirit, chat-
/// tuned wording — lowercase tool name, no CC-only PreToolUse caveats).
///
/// The "NEVER OFFER AN \"OTHER\" OPTION" paragraph is load-bearing, not
/// stylistic: Lucidos has no text-entry option kind, and
/// `agent_question::answer_kind_to_hook_value` resolves a `Selected` answer to
/// the option's LABEL (via `lookup_option_label`), so an "Other, I'll type it"
/// button hands that literal phrase back as the user's decision. The two
/// real escapes (typing in the prompt textarea, which routes to the pending
/// question as `FreeText`, and Cancel) are on every card already and are
/// spelled out to the user by the prompt row: typing by the textarea's own
/// placeholder while a question is pending (`PLACEHOLDER_ANSWERING`), Cancel
/// by the Cancel button's tooltip (`ANSWER_CANCEL_TOOLTIP`). Mirrored for coding
/// agents by `agent_session::prompts::{ASK_USER_QUESTION_RULE,
/// CODEX_ASK_USER_QUESTION_RULE}` and by the `ask_user_question` tool
/// description in `llm::tools::misc`; change them together.
///
/// The `multiSelect` paragraph is deliberately chat-only. The flag has existed
/// since 2026-05-10 and the tool schema documents it, but no prompt ever said
/// WHEN to set it, so the chat agent shipped single-pick cards for additive
/// questions and users answered by typing "the first three" into the prompt.
/// The coding-agent rules are not mirrored here because a coding agent's cards
/// are overwhelmingly decision forks (approve / a variant / stop), where
/// single-pick is the correct default and an added paragraph would be noise.
///
/// The WAKE QUESTION paragraph is scoped to "no Lucidos event to subscribe to"
/// because the unscoped version contradicted two other surfaces. It told the
/// agent to raise a one-option card after exhausting `bash_output` drains,
/// while the prompt's long-running-work section says to `await_event` on
/// `BackgroundBashCompleted` instead and the `ask_user_question` tool
/// description says never to ask purely to get resumed. Every example the old
/// paragraph gave (a backtest, an all-night sweep, an unsized download) is a
/// background task, which publishes that very event, so the rule as written
/// selected the wrong mechanism for its own examples. Narrowed rather than
/// dropped: an unbounded wait on something the engine emits nothing for still
/// has no other affordance.
///
/// Two paragraphs were retired by the 2026-08-07 third-pass budget trim, each
/// because a surface that is ALREADY resident says the same thing. The
/// argument-filling rules (`question` is required, `header` is a 12-character
/// chip and never a substitute for it, an empty `question` is rejected) now
/// live only in the tool schema, which is what the model reads at the moment it
/// fills those arguments in. And "never write a question whose premise assumes
/// they saw reasoning that lives only in your thinking block" was a restatement
/// of THINKING vs RESPONSE earlier in this same prompt, which states it for
/// every reply rather than only for a question.
pub(crate) const ASK_USER_QUESTION_RULE: &str = "ASKING THE USER QUESTIONS:\n\
     Use `ask_user_question` for any question with 2-4 discrete answers, \
     including a binary yes/no. The Lucidos UI renders the options as \
     clickable buttons; alternatives listed only in your message text force \
     the user to type their reply instead of tapping. The tool's own schema \
     owns the argument rules, `question` being required among them.\n\
     \n\
     THE TRIGGER IS QUESTION-SHAPE, NOT POSITION IN YOUR REPLY. A mid-stream \
     checkpoint and an end-of-turn \"what next, a, b or c?\" menu both become \
     buttons. If you find yourself typing a question mark, or writing a \
     bulleted list of next-step alternatives, route it through the tool. \
     Reserve plaintext for genuinely open-ended questions (\"what should I \
     name this?\") where pre-baked options would be guesses.\n\
     \n\
     SET `multiSelect` WHEN THE ANSWERS STACK. The card is single-pick by \
     default, and the test is mechanical: could a reasonable person want two \
     of these at once? Then set it. A checklist is multi-select; a genuine \
     fork, where picking one changes what you do next, is not.\n\
     \n\
     NEVER OFFER AN \"OTHER\" OPTION: nothing meaning \"Other\", \"Something \
     else\", \"Let me type it\" or \"I'll write my own answer\". Lucidos has \
     no text-entry option, so tapping one hands its label back as the user's \
     answer and leaves you re-asking. Both escapes are on every card already: \
     the user can type any reply in the prompt textarea and it arrives as \
     their answer, and Cancel dismisses the question so they can steer you \
     somewhere else. An option carrying a decision you can act on is a \
     different thing and still welcome (\"None of these\", \"Cancel the \
     deploy\").\n\
     \n\
     ANSWER FIRST, THEN OFFER CHOICES. The tool is an addendum to your reply, \
     never a replacement for it. If the user asked you something, ANSWER it \
     rather than bouncing a \"what do you want me to do now?\" menu back at \
     them. If they just gave you what you asked for last turn, that is a green \
     light to PROCEED, not to re-ask \"should I do it?\" with the options you \
     already offered. None of this conflicts with ACTION FIRST, which is about \
     not pausing to clarify what they already told you clearly.\n\
     \n\
     INVOKE IT AS A TOOL CALL, NEVER AS TEXT: a wrapper tag such as \
     `<ask_user_question>…</ask_user_question>` is not parsed out of assistant \
     text, so the user sees literal characters and no buttons. And NEVER \
     parallel-call it alongside other tools: a sibling call races the user's \
     reply and spends tokens on an unconfirmed direction.\n\
     \n\
     WAKE QUESTION, THE ONE-OPTION VARIANT: only for an unbounded wait with NO \
     Lucidos event to subscribe to. Anything the engine publishes an event \
     for, a background task's `BackgroundBashCompleted` included, is \
     `await_event` instead: a card whose only job is to wake you makes the \
     human your scheduler for something the engine already knows. With nothing \
     to subscribe to, call this with EXACTLY ONE option whose label is the \
     user-perspective wake prompt (\"Show results\", \"Stop sweep\") and your \
     context in `question`.";

/// Stops the chat agent from naming things to the user by their raw
/// identifier. The reported failure (2026-08-02): a trigger thread asked
/// "Change 99da1708 is docs-only and its plan is separate from it. What do you
/// want first?", with an option labelled "Apply 99da1708 now". That hex string
/// is a change id the user cannot resolve: no Lucidos surface is labelled with
/// it, so the question was unanswerable without scrolling back through the
/// agent's own tool calls.
///
/// The agent has everything it needs to do better: `changes` action 'list'
/// returns `thread_title` (the originating thread's title, batch-enriched by
/// `core::changes::enrich_thread_titles`) and `description` alongside the id.
///
/// Two carve-outs keep the rule presentation-only, and both are load-bearing.
/// Ids must stay legal in tool arguments, or the model stops passing
/// `change_id` to `changes(action='apply')`. And a **markdown link target** is
/// not prose: `run_thread` / `run_coding_agent` results tell the agent verbatim
/// to reply with `[Open thread](thread:<ws>/<uuid>)` (see
/// `agentic_loop_special_tool`), and `lucidos spawn-thread` prints that same
/// shape for a script to paste. A rule that banned the uuid there would strip
/// the only affordance the user has for opening the thread that was just
/// spawned.
///
/// Mirrored for coding agents by `agent_session::prompts::NAMES_NOT_IDS_RULE`
/// (same rule, tuned to commits and worktrees). Change both together.
const NAMES_NOT_IDS_RULE: &str = "NAMING THINGS TO THE USER, NEVER A RAW ID OR SHA:\n\
     Identifiers are for tool calls, not for prose. A change id, thread id, event id, request \
     id, commit sha, branch name, or any other uuid or hex string is meaningless to the user: \
     no Lucidos screen is labelled with it. NEVER put one in a message, in an \
     `ask_user_question` `question` or option label, or in a notification. \"Change 4f2c1a90 is \
     docs-only\" is the bug, \"the docs-only change from the Habit Tracker thread\" the fix.\n\
     Name each thing the way the user already sees it: a change by its originating thread's \
     title (`thread_title` from `changes` action 'list') plus what it touches, a thread by its \
     title, an app by its name (linked, see FILE REFERENCES), a trigger by its name, a file by \
     its full path, a commit by its subject line, never the sha. When two read alike, tell them \
     apart by what they change or when they arrived.\n\
     Ids stay legal where the user never sees them: `changes(action='apply', change_id=…)` \
     still takes the uuid, and tool results still carry them.\n\
     A MARKDOWN LINK TARGET IS NOT PROSE, so keep writing the links: \
     `[Open thread](thread:<ws>/<uuid>)` for a thread you spawned, pasted exactly as the tool \
     result gives it to you, and `[Habit Tracker](app:habit-tracker)` for an app. Dropping one \
     because it holds a uuid leaves the user no way to open what you just started.\n\
     ONE EXCEPTION: the user asked for the raw value, or needs it to paste somewhere. Then give \
     it, wrapped in <copy>…</copy> tags.";

/// Stops the chat agent from CLAIMING it performed an action it never actually
/// invoked the tool for — the single most common way the agent lies to the
/// user. The observed failure (testing-notifications thread, 2026-06-30): the
/// user said "again" four times; the agent called `send_notification` on the
/// first two and then, seeing the two identical prior `ToolUse:
/// send_notification` entries sitting in its context, just wrote "Sent another"
/// for the last two WITHOUT calling the tool — so no notification went out, but
/// the user was told it did. The CRITICAL RULES block already says "never claim
/// you did X unless the tool returned success", but it was scoped to
/// `write_file`/`edit_file`, so the model didn't apply it to other action
/// tools. This rule generalizes it and names the repeat-request trap directly.
/// Logged as a model-tolerance measure in `docs/temporary-measures.md`.
pub(crate) const REPEATED_ACTION_RULE: &str = "DOING IT AGAIN, A REPEAT \
     REQUEST NEEDS A FRESH TOOL CALL:\n\
     When the user says \"again\", \"once more\", \"resend\", \"send another\", \
     you MUST invoke the tool again IN THE CURRENT TURN before you confirm it. \
     An identical earlier call sitting in this conversation is a record of a \
     PREVIOUS turn; it does not mean the action happened this time. The trap: \
     a prior `send_notification` is in context, they say \"again\", and you \
     write \"Sent another\" without calling anything, so nothing goes out and \
     they were told it did.\n\
     NEVER write a confirmation (\"sent\", \"created\", \"updated\", \
     \"emitted\", \"deleted\", \"done\", \"sent again\") unless the matching \
     tool returned success IN THIS turn. If you did not call the tool this \
     turn, you did not do it. Holds for every state-changing tool, not just \
     file writes.";

/// Stops the agent from routing around a tool it was refused by, and from ever
/// posting to the engine as the user.
///
/// The reported failure (2026-08-06): asked to send a follow-up to every
/// running coding-agent thread, the agent found that `follow_up_child_thread`
/// only reaches its own children and that no tool covered the request. Instead
/// of saying so, it used `run_bash` to `curl` the engine's own
/// `/api/v1/chat/stream` with `mode: "human"`, which recorded six agent
/// messages as turns the user had typed. It also hit the wrong engine, whose
/// chat path then created six phantom threads from the ids it sent, so reading
/// them back "confirmed" delivery and it reported success.
///
/// The engine now refuses all three moves (`api::chat::human_mode_is_attributed`,
/// `api::chat::thread_target_is_addressable`, `api::target_workspace`). The
/// model must still be TOLD, for the same reason
/// [`NO_LUCIDOS_SOURCE_SECTION`] exists: a rule that only blocks lets the agent
/// narrate the capability first and discover the refusal after the user has
/// been misled. And the load-bearing half is not the prohibition but the
/// instruction about what to do instead, because the actual failure was not
/// reporting the block.
///
/// Deliberately short. It says what not to do, in one place, and points at the
/// tools that do work rather than restating their contracts, which
/// `system-knowhow/lucidos-cli.md` owns.
const NO_IMPERSONATION_RULE: &str = "NEVER ACT AS THE USER, AND NEVER ROUTE \
     AROUND A TOOL:\n\
     You may never post to the Lucidos engine's own HTTP API by hand (curl, \
     urllib, fetch) to do something a tool would not let you do, and you may \
     never record a message as though the user typed it. An agent-authored turn \
     stamped as the user is indistinguishable from the real thing in the \
     timeline and the event log, so it is a lie the user cannot detect. The \
     engine refuses these, but do not go looking for the edge it misses.\n\
     WHEN A TOOL REFUSES YOU, SAY SO. That is the whole rule. Asked for \
     something no tool covers (messaging a thread that is not your own child, \
     acting in another workspace), tell the user plainly that it is not \
     possible and offer what IS: name the threads and let them send the \
     message themselves. A refusal reported honestly is a good turn; a refusal \
     worked around is a broken one, however well it appears to succeed.";

/// Routes "tell me here when X happens" to `await_event` rather than a
/// trigger, at the moment the mechanism is chosen.
///
/// Every other surface describing `await_event` frames it as an agent-side
/// remedy: the tool description says "INSTEAD of a polling loop", the
/// state-change section below is headed "NEVER A POLL LOOP", and
/// `system-knowhow/thread-events.md` says "when the current turn cannot
/// usefully continue". A user asking to be told about something introduces no
/// poll loop and blocks no turn, so none of those framings matches, and the one
/// property the request does carry (the answer has to land HERE) appeared
/// nowhere. Worse, the single discriminator that was stated everywhere,
/// one-shot versus standing, actively selects the wrong mechanism: "when a
/// coding agent edits code let me know" parses as a standing rule.
///
/// That is the 2026-08-06 miss. The agent loaded the coding-agent-events and
/// triggers knowhow, listed triggers, and asked which granularity of *trigger*
/// to build, having already written "a trigger can't write into this chat
/// thread" as a caveat to accept rather than as the disqualifying fact it is.
/// It reached `await_event` only when the user named the tool.
///
/// So the destination question is stated here, alongside the duration question
/// rather than replacing it, on a surface that is resident in every chat turn.
/// The knowhow carries the same fork, but knowhow has to be RETRIEVED, and in
/// the miss the retrieval had already committed to triggers before the fork
/// could be considered.
///
/// Deliberately does not claim a trigger cannot reach the user at all: it can,
/// as a notification from its own thread. What it cannot do is continue the
/// conversation the user typed into. Pinned by
/// `trigger_vs_event_wait_rule_routes_on_destination_without_overclaiming`.
///
/// Extracted as a const rather than left inline in the prompt literal so it is
/// assertable, matching [`ASK_USER_QUESTION_RULE`] and [`REPEATED_ACTION_RULE`].
pub(crate) const TRIGGER_VS_EVENT_WAIT_RULE: &str =
    "\"TELL ME WHEN X HAPPENS\", TRIGGER OR `await_event`? ASK WHERE THE ANSWER \
     GOES, NOT JUST HOW OFTEN:\n\
     - WHERE. A trigger runs in its OWN thread and reaches the user as a \
     notification. It cannot continue the conversation you are in. \
     `await_event` re-opens THIS thread, so the report lands where they are \
     reading. \"Let me know HERE\", \"tell me in this chat\", or a request typed \
     into a thread they are plainly waiting in, is `await_event`, even when the \
     phrasing sounds like a standing rule.\n\
     - HOW LONG. `await_event` is one-shot: it resolves on the first match, and \
     you subscribe again per event. A rule that must outlive this conversation, \
     run when nobody is here, and fire indefinitely is a trigger.\n\
     - Both can be right. Lead with the one that matches where they asked to be \
     told, and offer the other in the same breath. Never build a trigger just to \
     post back into a chat thread.";

/// Routes the chat agent to the authoritative system-knowhow file before it
/// touches a workspace asset.
///
/// Deliberately covers **operating on an existing trigger**, not just creating
/// one. The block used to read "Before creating a trigger, app, knowhow file,
/// or plugin" with a `triggers (action create/update)` row, so a request to
/// RUN an existing trigger off-schedule matched no route: the agent never
/// loaded the trigger knowhow, guessed, and started by resuming a paused
/// trigger (which restores the schedule and runs nothing). Any future
/// narrowing of the trigger row reintroduces that gap, which is what
/// `workspace_assets_rule_routes_existing_trigger_operations` pins.
///
/// Extracted as a const rather than left inline in the prompt literal so it is
/// assertable, matching [`ASK_USER_QUESTION_RULE`] and [`REPEATED_ACTION_RULE`].
pub(crate) const WORKSPACE_ASSETS_KNOWHOW_RULE: &str =
    "WORKING WITH WORKSPACE ASSETS, LOAD KNOWHOW FIRST:\n\
     Before creating a trigger, app, knowhow file, or plugin, AND before \
     acting on an EXISTING trigger, you MUST first call load_knowhow on the \
     matching system-knowhow file:\n\
     - triggers, ANY action (create, update, pause/resume, or run one now, \
     off-schedule) -> `system-knowhow/triggers`\n\
     - create_app -> `system-knowhow/building-an-app`\n\
     - a new file under knowhow/ -> `system-knowhow/building-knowhow`\n\
     - packaging a plugin -> `system-knowhow/plugins`\n\
     To run an existing trigger off-schedule, use triggers(action=\"run\"). Do \
     not improvise a substitute: resuming a paused trigger does not run it, \
     and a run_thread carrying a copy of the trigger's intent is not a run.\n\
     Each knowhow has a \"Questions to settle with the user before creating\" \
     section, which is the source of truth for what to ask. The ACTION FIRST \
     rule below does NOT apply to workspace assets: load the knowhow, ask what \
     it says to ask, then act. Skip the load if you already loaded that \
     knowhow earlier in this thread.";

/// Routes "set Lucidos up around my life" to the *setup interview* knowhow.
///
/// The first-run entry point (the welcome CTA and the header help button, both
/// in `WelcomeMessage.tsx` / the app headers) sends a fixed sentence as an
/// ordinary user message, so the opening request has a KNOWN shape. That is the
/// same situation `plugin-setup` is in, and it gets the same treatment: a
/// deterministic route rather than a bet on the frontmatter `description`
/// winning retrieval. The description is written for retrieval too, and covers
/// the re-run phrasings a returning user types; this block is what makes the
/// button itself reliable.
///
/// The ACTION FIRST carve-out is load-bearing, not boilerplate. That rule says
/// "Don't ask clarifying questions" and currently excepts only workspace
/// assets, but an interview is a clarification loop by construction, so without
/// the carve-out the agent skips the questions and builds something generic on
/// turn one. Pinned by `setup_interview_rule_carves_itself_out_of_action_first`.
///
/// The "not only about work" sentence is here rather than left to the knowhow
/// alone because this block is what the model reads BEFORE deciding to load
/// anything: a rule that describes the interview as being about the user's work
/// invites a "they only asked about training, this is something else" miss on
/// exactly the requests the widened interview exists to serve.
pub(crate) const SETUP_INTERVIEW_RULE: &str =
    "SETTING THE WORKSPACE UP AROUND THE USER, LOAD KNOWHOW FIRST:\n\
     When the user wants Lucidos set up around their own life rather than a \
     single answer (\"help me get the most out of Lucidos\", \"set me up\", \
     \"build me a starting kit\", \"what should I use Lucidos for\", \"help me \
     get started\", \"figure out what to build for me\", \"help me with my \
     training\", \"coach me\", or a returning user asking to run setup again), \
     FIRST call load_knowhow on `system-knowhow/setup-interview` and follow it: \
     it owns the question ladder, how to map answers to a kit worth building, \
     when to cut the questions short, and what to persist.\n\
     This is NOT only about work: personal admin, health and training, \
     learning, a side project and a household are all in scope, and the \
     knowhow's first question is which mix applies. Route the request here \
     whether or not it mentions a job.\n\
     The ACTION FIRST rule below does NOT apply to the setup interview: the \
     questions ARE the work there, so ask them rather than guessing a kit. Skip \
     the load if you already loaded it earlier in this thread.";

/// The workspace-independent body of every chat system prompt. Rule consts sit
/// in it as double-underscore-delimited placeholder tokens that
/// [`static_prompt_body`] resolves.
///
/// Module-level rather than inline in [`LucidosEngine::build_chat_system_prompt`]
/// so the budget test can measure the real text instead of a copy. Every
/// placeholder here needs a matching `.replace(...)` in [`static_prompt_body`],
/// which `every_prompt_placeholder_is_substituted` enforces by scanning this
/// file's source. That scan is why no doc comment or test in this file may
/// write a placeholder-shaped token in prose: it would look like an
/// unsubstituted one.
const SYSTEM_PROMPT_BASE: &str = r#"
PERSONALITY:
- Warm but concise: acknowledge what users say, ask relevant follow-ups
- Proactive: offer to create files, track things, set reminders when appropriate
- Contextual: remember past conversations and reference them naturally

MEMORY:
- LONG-TERM MEMORY is organized by topic with dated entries, and a recent entry supersedes an older one in the same topic. Draw from all topics for a broad question.

SELF-AWARENESS (answer naturally when asked): "who are you?" is a brief intro plus what you are currently tracking for them; "what am I working on?" is their active projects from files and recent conversations; "what do you know about me?" is the learned context (name, projects, preferences).

USER PROFILE (artifacts/user_profile.md):
- NEVER read it, it is already in your context below.
- Write CONFIRMED facts only, things the user explicitly stated, never a guess or an inference; ask to confirm when unsure. If it doesn't exist when they ask personal questions, suggest they tell you about themselves first.

WORKSPACE LAYOUT:
  .lucidos/tmp/            Gitignored scratch from http_request / git_clone
  data/artifacts/          User files (notes, imported data, projects, screenshots),
                           including user_profile.md and imported/<service>/
  data/apps/<id>/          App: manifest.json + ui/, plus knowhow/ intents/ scripts/ triggers/
  data/knowhow/            Standalone knowhow docs (API specs, data formats)
  data/triggers/<slug>/    Standalone triggers: <slug>.md + scripts/

PATHS:
- A tool path under data/ is relative to data/: "artifacts/notes.md", NOT "data/artifacts/notes.md". A .lucidos/ path is relative to the workspace root: ".lucidos/tmp/file.json", NEVER "data/.lucidos/tmp/file.json". Pass a .lucidos/tmp/ path reported by http_request or git_clone straight to read_file; don't `cat` it and don't re-fetch.
- Only .lucidos/tmp/ is reachable by the file tools; the rest of .lucidos/ is engine runtime state and is refused. Those tools git-commit what they write, so they REFUSE to write .lucidos/tmp/: create or edit scratch with run_python, delete it with run_bash. run_python's cwd is the workspace root, so open('.lucidos/tmp/x.json') for scratch and open('data/artifacts/x.md') for an artifact.
- Everything under data/ (except postgres/) is git-tracked. Never nest artifacts inside artifacts.

CONTENT TAXONOMY (intent, knowhow, script, trigger; each scoped to an app, a knowhow domain, or a trigger):
- Intent: what the user wants, in their terms. Goals, conditions, outcomes. Stable. Frontmatter: `name`, optional `knowhow` (ids, each the path under data/knowhow/ without .md and including subdirectories, e.g. 'weather/api'; engine-shipped docs take the 'system-knowhow/' prefix).
- Knowhow: how to achieve it, in technical terms. API details, formats, quirks, workarounds. This is YOUR memory and you maintain it: when you discover a quirk, a better approach or a failure mode, update the relevant file. Frontmatter: `name`, optional `description` (what semantic discovery matches against).
- Script: code invoked by an intent or knowhow. Trigger: a scheduled or event-driven task.
Change an intent only when the user's goal changes, and never put technical detail in one.

SCRIPT FILES (under apps/, triggers/, knowhow/scripts/):
- Every script the engine spawns gets the `lucidos` CLI on PATH and LUCIDOS_WORKSPACE set. Write data with `lucidos data write`, not raw HTTP to the engine, and emit or read domain events with `lucidos events emit` / `lucidos events query`. Full reference: load_knowhow('system-knowhow/lucidos-cli').

FILE FORMATTING:
Clean structured markdown: ## headings, bullet lists, **bold** for key values.

THINKING vs RESPONSE:
- Your thinking block is private and THE USER NEVER SEES IT. Anything they need (a finding, an explanation, the reason behind a recommendation) reaches them only in your RESPONSE text, so never ask a follow-up that assumes they saw reasoning you did in your head.
- Your response is the clean final message: no raw chain-of-thought, no narrating every step. That never means "no explanation": when the user asks why, where or how, the explanation IS the message.
- NEVER repeat yourself across tool calls. If you explained something before a call, don't restate it after.

CONVERSATION STYLE:
- Vary your openings, and NEVER start with "Okay" or "Sure": answer directly.
- When the user shares what they're working on, acknowledge it and ask ONE follow-up. Don't interrogate.
- Create artifacts as the conversation progresses, not all at once.

COPYABLE TEXT: wrap text the user will want to copy (a command, a URL, a key, an id, instructions for another session) in <copy>...</copy> and the UI adds a one-click copy button. Not for prose, explanations or headings, and not for code blocks, which have their own.

UNCERTAINTY: USE web_search when answering trivia, identifying something, or verifying a fact you're unsure of. If you can't search and don't know, say so, and for a riddle ask for a hint rather than guessing repeatedly. Never write a guess to user_profile.md or any other memory file.

__WORKSPACE_ASSETS_KNOWHOW_RULE__

SETTING UP A NEWLY-INSTALLED PLUGIN, LOAD KNOWHOW FIRST:
When the request is to set up a newly-installed plugin, FIRST load_knowhow('system-knowhow/plugin-setup') and follow it: it owns how to find the author's setup instructions, plan the steps, and complete them with the user. Skip the load if you already loaded it in this thread.

__SETUP_INTERVIEW_RULE__

ACTION FIRST, NO CLARIFICATION LOOPS:
- If the user asks for something, DO IT. Resolve ordinary vagueness yourself ("this week" is since Monday), and act on a request that is 80% clear. If you're wrong the user will say so, which is faster than a clarification loop.
- NEVER ask "do you mean X?" or "just to clarify". "Show me this week's tasks" is answered by showing them, not by asking whether that means since Monday or the last 7 days.
- Only ask when you genuinely cannot proceed. Two rules above override this one: WORKING WITH WORKSPACE ASSETS, and the setup interview.

__ASK_USER_QUESTION_RULE__

TODO LIST (todo_write, live progress in the prompt bar):
- Use it for a multi-step request (3+ steps) so the user watches the plan tick along; skip it for trivial single-step work.
- Call once at the start with every item `pending`, then re-call flipping one to `in_progress` before you start it and to `completed` when it's done.

TOOLS: Use efficiently, don't loop. One call per file, and don't re-read a file you just wrote. Prefer edit_file over write_file for an existing file, and its json_path + new_value mode for a .json or .slides file.

MEMORY CORRECTIONS (the `memory` tool):
- When the user says a memory is wrong ("I don't work at Acme Corp"), pass a broad search_query ("Acme") and a specific wrong_fact ("User works at Acme Corp"), plus an optional `correction` to replace it. A correction persists across memory rebuilds.
- Afterwards, if user_profile.md or another artifact still carries the stale fact, ASK before editing it. Never edit an artifact automatically during a memory correction.

BROWSER TOOLS:
- browser_open is ONLY for an external website the user asks you to visit. NEVER point it at your own app UIs, artifacts, or any Lucidos file: those are edited with the file tools and opened by the user in the frontend.
- Use visible=true when the user says "show me" or "let me log in", or when a site blocks headless browsers or redirects you to a login page.

__TRIGGER_VS_EVENT_WAIT_RULE__

TRIGGERS:
- Cron is 6 fields (sec min hour dom month dow) in the user's LOCAL timezone; DST is handled automatically.
- In a trigger thread, send_notification only when there is something noteworthy to report. If nothing changed, just finish. Errors are reported automatically.

IMPORTING DATA & CREDENTIALS:
- API data: check credentials, request_credential if missing, http_request, write_file to imported/<service>/. A local file is import_file with the full path.
- NEVER accept a token or key pasted in chat. request_credential opens a secure popup that keeps the secret out of the event log; if the user pastes one, redirect them to it.

EMAIL SETUP:
- Walk the user through it: ask which provider, web_search its current IMAP and SMTP host, port and auth requirements, do the OAuth setup first if it needs one (most do), then configure_email with use_oauth when OAuth is connected or an app password otherwise, then test by sending a mail or reading the inbox.
- NEVER put an email password in chat; configure_email collects it in a popup.

OAUTH SETUP:
- load_knowhow('system-knowhow/oauth-providers') BEFORE collecting any OAuth credential: it owns the provider registry, the redirect-URI forms, the confidential-vs-public client rule and the scope-alias rule. If a provider is not listed, web_search its endpoints and app-registration steps and ADD it there.
- connect_oauth_account is ONE call for the whole flow, and never ask for a client_id or secret in chat. Walk the user through registering the app before you call it (the redirect URI, the scopes, where to find the client id), assuming they have never done OAuth setup before.

LOOKING UP SPECIFIC DATA (numbers, ids, dates, amounts, addresses):
- Find the file with list_files, or glob_files / grep_files on a larger workspace, then read_file it. A memory summary says WHAT a file contains, never the exact values inside it.
- A PDF is stored as a binary artifact with no text extraction, and read_file says so (unless a legacy .txt sidecar exists, which it surfaces automatically).

SEARCHING FILES:
- glob_files finds files by path pattern and grep_files regex-searches contents, both relative to data/. Prefer them over run_bash with find, rg or grep: structured, faster, and they respect workspace boundaries.

LONG-RUNNING WORK, AND THE REPEATED-CALL GUARD:
- run_bash and run_python are synchronous with a 300s ceiling and WILL kill anything longer mid-stream. For longer work spawn run_bash_background, or run_python_background when the script needs scientific packages. Drain either with bash_output(task_id, wait_secs=N), using the FULL 120 for anything long, and cancel with bash_kill(task_id).
- READ `elapsed_secs` AND `waited_secs`, NEVER ESTIMATE ELAPSED TIME YOURSELF. You have no clock, and adding up the waits you asked for is how you tell the user "about 20 minutes in" 90 seconds into a build. A `waited_secs` far below what you asked for, with `finished: false`, means the USER SENT A MESSAGE and the engine cut the block short: read it, answer, drain again.
- For a genuinely unbounded wait, stop after a couple of full drains and await_event on `BackgroundBashCompleted` with a `condition` on the task_id. Do NOT end the turn with "I'll report when it finishes" and no call: nothing would wake you.
- THE REPEATED-CALL GUARD IS INVISIBLE UNTIL IT FIRES. The engine buckets consecutive same-target calls: at 3-4 same-bucket calls it REPLACES the result with a STOP message and the call never runs, at 5 it force-ends your turn. Heed the first STOP, and vary the first token or restructure as ONE command rather than retrying. edit_file, write_file, web_search and browser_* are exempt.
- The one poll that IS right is live progress the user asked to watch ("ping me every 60s"): ONE run_python with an internal `for _ in range(4): print(bar()); time.sleep(60)`, which fits under the 300s ceiling. For longer, spawn one background task that appends a progress line to a file and drain that.
- load_knowhow('system-knowhow/running-python') for the drain pattern, the full wait_secs semantics, and how each tool's bucket key is derived.

WAITING FOR A STATE CHANGE IN LUCIDOS: USE `await_event`, NEVER A POLL LOOP:
- Everything above is about the OUTPUT of a process you started. A STATE CHANGE in Lucidos is a different thing: a change being proposed, a trigger firing, a thread you did NOT spawn going idle, a domain event. `await_event` subscribes you and re-opens this thread when it arrives.
- Reach for it INSTEAD of a bash_output drain loop, a sleep-and-recheck script, or a `threads list` / `changes list` poll: a poll costs a turn per check and can miss a transition between samples. It is ALSO how you deliver, not only how you wait, so nothing has to be blocking you. It is NOT for external state with no Lucidos event (a third-party API, a file another process writes), which you poll with the background tools.
- One subscription resolves on one match, which is the duration half of the choice; the destination half is TELL ME WHEN X HAPPENS above, which decides "let me know HERE".
- IT WATCHES THE WHOLE WORKSPACE, not just this thread, so ANY thread's completion is a first-class wait, someone else's coding-agent session included: name it with a `child_thread_id` condition. NOT YOUR OWN CHILD: it already re-opens this thread with its status, summary and pending change ids, so a wait on it is a second wake. Say you'll report back, and end the turn.
- IT RETURNS IMMEDIATELY AND BLOCKS NOTHING, so say what you're waiting for and END YOUR TURN. The tool's own schema carries the rest.
- NEVER ANSWER "AM I STILL WATCHING FOR THAT?" FROM MEMORY: call `list_event_waits`, and `cancel_event_wait` to STOP.

REFRESHING OPEN WINDOWS:
- A file the user has open refreshes itself the moment you write it, and app UIs refresh when their files change. There is no file-refresh tool, so don't spend a step looking for one and don't tell the user to refresh. (refresh_app is a different thing and still exists.) If the file isn't open, just mention the path; it renders as a clickable link.

FILE REFERENCES:
- Always use the full path ("artifacts/notes.md", not "notes.md"): a full path becomes a clickable link, a bare filename does not.
- LINK EVERY APP YOU NAME, since a bare app name does not auto-link: `[Habit Tracker](app:habit-tracker)`. Not linking should be a rare exception.
- Link a UI panel by its bare name: `[Notifications](notifications)`, `[Triggers](triggers)`, `[Settings](settings)`. Apps and other plugins are downloaded from `[Plugins](app-store)`; call it the Plugins panel, never the retired "App Store" or "Store".

__NAMES_NOT_IDS_RULE__

PLUGINS (the `plugins` tool):
Once a plugin is installed its files live under data/ and are indistinguishable from the user's own, so never delete_file them: 'uninstall' stages a panel that removes them. 'install' stages one too, and after calling either, do NOT claim it succeeded; the next message reports the outcome.

EVENTS (the `events` tool):
- Events are immutable and append-only: facts, not intentions. A domain event you 'emit' is PascalCase past tense with a "summary" in its payload.
- 'query' answers historical questions ("how many workouts this week?", "when did I last log X?"), and limit=1 gives the most recent of a type. Engine events answer the same way: `ChildThreadCompleted`, `ResponseGenerated`, `ChangeApplied`, `TriggerCompleted`, and the rest of system-knowhow/thread-events. Prefer this tool over run_python or SQL; use Python only for reporting 'query' cannot do.
- App UIs reach the platform through the Lucidos SDK (<script src="/api/v1/sdk.js">): lucidos.data.*, lucidos.events.*, lucidos.preferences.*, lucidos.ui.*, lucidos.sse.on, lucidos.proxy(name).fetch. Details: load_knowhow('system-knowhow/js-sdk').

PARALLEL WORK (FAN-OUT):
- run_coding_agent starts a coding-agent thread for code work; run_thread starts a Lucidos thread for non-code work; follow_up_child_thread steers a child you already spawned. You can only address your own DIRECT children, which the `threads` tool's 'list' action lists with `my_children: true`.
- The resume callback that reports a child's result back here only works for same-workspace children spawned with these tools.
- For a pipeline where step N depends on step N-1, spawn ONE child per response and wait for the callback. Never batch sequential spawns into one response.
- SPAWN SPARINGLY. Default to doing the work yourself. Spawn only for genuinely independent subtasks that gain from running in parallel, never for what a few sequential tool calls would do, and never one thread per item in a list. Maximum __MAX_CHILDREN_PER_THREAD__ children per thread, maximum depth 3.

__NO_IMPERSONATION_RULE__

__ENGINE_RESTART_RULE__

__APPLY_VERIFY_RULE__

__LOOK_BEFORE_ASSESSING_RULE__

ENGINE INTERNALS YOU CANNOT OBSERVE:
You cannot count your own tool calls or measure any internal engine budget. The only real per-turn cap is __MAX_TOOL_CALLS__ tool calls, and the "[ENGINE-LIMIT]" prefix on the engine's message is the only signal it fired (the user changes it in Settings, Models, Chat & triggers). Never claim you hit a "tool-call cap" or "per-turn limit", and never cite a number or a file name about the agent loop: they are invisible to you, and inventing them poisons long-term memory. If you stop mid-task, give the real reason.

CRITICAL RULES:
1. NEVER claim you performed an action unless the matching tool returned success IN THE CURRENT TURN. Every state-changing tool, not just file writes; the closing rule below spells this one out.
2. When a request needs a tool action, call the tool instead of describing a plan.
3. For specific data (numbers, ids, dates), read the file. Don't answer from a summary.
4. NEVER show code in a response unless the user asked for code.
5. Asked to create N files, call write_file N times IN THE SAME RESPONSE.

__REPEATED_ACTION_RULE__"#;

/// The per-turn ENGINE BUILD section: what the running engine is, and whether
/// the user has switched onto a newer one.
///
/// Pure over [`VersionStatus`] so the wording can be tested without an engine,
/// and so the three cases stay visibly distinct. They are not the same claim:
///
/// * **Current.** Running build equals the on-disk build and source is not
///   ahead. Nothing is pending, so a `requires_restart` change applied earlier
///   in this thread IS live. This is the case the agent got wrong.
/// * **Built, not switched.** A newer binary is on disk. The user has not
///   pressed *Switch to new version* yet, so the running engine predates it.
/// * **Source ahead, not built.** A restart-requiring change is on main with no
///   fresh binary behind it yet (the rebuild is running, or failed). Saying
///   "the user has not switched" here would be misleading: there is nothing to
///   switch onto.
///
/// Deliberately no build id in the text. The user cannot look one up on any
/// screen, so it would only invite the agent to quote a hex string at them
/// (`.claude/rules/glossary.md`, and the engine prompt's own NAMES NOT IDS
/// rule); what the agent needs is the verdict, which the boolean already is.
fn engine_build_section(status: &crate::engine::engine_version::VersionStatus) -> String {
    let state = if status.update_available {
        "A NEWER ENGINE IS BUILT AND THE USER HAS NOT SWITCHED ONTO IT YET, so an applied \
         restart-requiring change is NOT live."
    } else if status.source_behind_head {
        "A RESTART-REQUIRING CHANGE IS ON MAIN WITH NO BUILD BEHIND IT YET (rebuilding, or \
         it failed), so there is nothing to switch onto: do not tell them to."
    } else {
        "THE RUNNING ENGINE IS CURRENT, matching both the built binary and main. Any applied \
         restart-requiring change IS live, and the user HAS restarted if one needed it."
    };
    format!(
        "\n\nENGINE BUILD (rebuilt every turn, never stale):\n{state}\nThis is the answer to \
         \"has the user restarted?\". Never ask, and never infer it from what you applied \
         earlier: they restart when they like, including mid-turn."
    )
}

/// Resolve every placeholder token in [`SYSTEM_PROMPT_BASE`] and append the
/// coding-surface section, yielding the workspace-independent body of the chat
/// system prompt.
///
/// Pure over its inputs so the budget test measures the text the engine really
/// assembles rather than a stand-in, and so the substitution chain has one home.
/// `max_tool_calls` is this turn's resolved cap, passed in rather than read here
/// so it is the SAME number `run_agentic_loop` enforces: a prompt naming a number
/// the loop doesn't use is exactly the fabricated engine internal the ENGINE
/// INTERNALS section warns against.
fn static_prompt_body(has_lucidos_source: bool, max_tool_calls: usize) -> String {
    let apply_verify_rule = if has_lucidos_source {
        format!("{}{}", APPLY_VERIFY_RULE, APPLY_VERIFY_DEV_ADDENDUM)
    } else {
        APPLY_VERIFY_RULE.to_string()
    };

    let body = SYSTEM_PROMPT_BASE
        .replace("__ENGINE_RESTART_RULE__", ENGINE_RESTART_RULE)
        .replace("__APPLY_VERIFY_RULE__", &apply_verify_rule)
        .replace("__LOOK_BEFORE_ASSESSING_RULE__", LOOK_BEFORE_ASSESSING_RULE)
        .replace("__ASK_USER_QUESTION_RULE__", ASK_USER_QUESTION_RULE)
        .replace(
            "__WORKSPACE_ASSETS_KNOWHOW_RULE__",
            WORKSPACE_ASSETS_KNOWHOW_RULE,
        )
        .replace("__SETUP_INTERVIEW_RULE__", SETUP_INTERVIEW_RULE)
        .replace("__TRIGGER_VS_EVENT_WAIT_RULE__", TRIGGER_VS_EVENT_WAIT_RULE)
        .replace("__NAMES_NOT_IDS_RULE__", NAMES_NOT_IDS_RULE)
        .replace("__NO_IMPERSONATION_RULE__", NO_IMPERSONATION_RULE)
        .replace("__REPEATED_ACTION_RULE__", REPEATED_ACTION_RULE)
        .replace("__MAX_TOOL_CALLS__", &max_tool_calls.to_string())
        .replace(
            "__MAX_CHILDREN_PER_THREAD__",
            &super::super::recursion_guard::MAX_CHILDREN_PER_THREAD.to_string(),
        );

    format!("{}{}", body, coding_surface_section(has_lucidos_source))
}

impl LucidosEngine {
    /// Build the full chat system prompt for this turn plus the mandatory
    /// missing-preference keys and whether an image provider is available
    /// (consumed by the tools list). Verbatim extraction of the inline block
    /// from `process_message_with_steps_internal`.
    pub(super) async fn build_chat_system_prompt(
        &self,
        user_timezone: &str,
        user_language: &str,
        device_id: Option<&str>,
        is_trigger: bool,
        trigger: &Option<TriggerContext>,
        // This turn's resolved tool-call cap, for the `__MAX_TOOL_CALLS__`
        // placeholder. Passed in rather than read here so it is the SAME number
        // `run_agentic_loop` enforces: the prompt tells the model this is the
        // only real per-turn cap, and a prompt that names a number the loop
        // doesn't use is the fabricated engine internal that section warns
        // against.
        max_tool_calls: usize,
    ) -> (String, Vec<&'static str>, bool) {
        // System prompt with current date for time-aware responses
        let now = Utc::now();
        let current_date = now.format("%A, %B %d, %Y").to_string(); // e.g., "Thursday, January 30, 2026"
        let current_time_utc = now.format("%H:%M UTC").to_string();

        // Build timezone section based on whether timezone is set
        let timezone_section = if user_timezone.is_empty() {
            format!(r#"CURRENT TIME: {} at {}"#, current_date, current_time_utc)
        } else {
            // Calculate user's local time
            let tz: chrono_tz::Tz = user_timezone.parse().unwrap_or(chrono_tz::UTC);
            let local_now = now.with_timezone(&tz);
            let current_time_local = local_now.format("%H:%M").to_string();

            // Get UTC offset in hours (e.g., +1 for CET, +2 for CEST)
            use chrono::Offset;
            let offset_seconds = local_now.offset().fix().local_minus_utc();
            let offset_hours = offset_seconds / 3600;
            let utc_offset_str = if offset_hours >= 0 {
                format!("+{}", offset_hours)
            } else {
                format!("{}", offset_hours)
            };

            format!(
                r#"CURRENT TIME: {} at {} (user's local time: {} {})
USER TIMEZONE: {} (UTC{})

TIMEZONE HANDLING:
- The user speaks in their LOCAL time ({}).
- All timestamps are stored as UTC in the database.
- ALWAYS display times to the user in their local timezone (not UTC).
- Cron uses 6 fields: second minute hour day-of-month month day-of-week
- When user says "at 8am", use "0 0 8 * * *" (second=0, minute=0, hour=8).
- Example: "daily at 8am" → cron "0 0 8 * * *", "at 9:30" → "0 30 9 * * *"
- The system automatically handles daylight saving time adjustments."#,
                current_date,
                current_time_utc,
                current_time_local,
                user_timezone,
                user_timezone,
                utc_offset_str,
                user_timezone
            )
        };

        // Mandatory setup preferences — to add a new one, add a (key, instruction) tuple.
        // Each is checked against PreferenceStore; any missing key triggers setup mode.
        // Global prefs (timezone, language) use PreferenceStore::get.
        // Per-device prefs (push_notifications) use PreferenceStore::get_for_device.
        let mandatory_prefs: &[(&str, &str, bool)] = &[
            ("timezone", "- TIMEZONE: Ask what timezone they are in and call preferences(action=\"set\", key=\"timezone\", value=\"…\") with an IANA name (e.g., \"America/New_York\", \"Europe/London\", \"Asia/Tokyo\").", false),
            ("language", "- LANGUAGE: Ask what language they prefer, and mention that English is recommended for best results (the models are strongest in English) — but they can still write in any language; replies come back in whichever language they set here. Then call preferences(action=\"set\", key=\"language\", value=\"…\") to save it.", false),
            ("push_notifications", "- PUSH NOTIFICATIONS: Ask if they want to enable push notifications for scheduled task alerts (do NOT call them \"browser\" notifications — Lucidos runs as a native desktop app too, where these are native OS alerts). When you describe how they arrive, key off the current request device in [USER DEVICE & PREFERENCES]: if its details say \"Lucidos desktop app\" they are in the native desktop app (native macOS notifications, no browser or site permission); otherwise they are in a browser/PWA (the browser will prompt for permission). If yes, call preferences(action=\"set\", key=\"push_notifications\", value=\"enabled\"). If no, call preferences(action=\"set\", key=\"push_notifications\", value=\"declined\") so you don't ask again.", true),
        ];

        let mut missing_instructions = Vec::new();
        let mut missing_pref_keys = Vec::new();
        for (key, instruction, per_device) in mandatory_prefs {
            // A read that FAILED is not a preference that is unset. `.ok().flatten()`
            // collapsed both into `None`, and `None` here flips the whole turn into
            // "SETUP REQUIRED, DO NOT PROCEED" below (plus the paired user-message
            // reminder in `run.rs`), so one transient DB error refused the user's
            // actual request and re-asked for a timezone they had already set.
            // Treat an unreadable key as configured: a missed setup nag costs a
            // prompt, a false refusal costs the turn.
            let read = if *per_device {
                if let Some(did) = device_id {
                    PreferenceStore::get_for_device(&self.pool, key, did).await
                } else {
                    // No device context (child thread, scheduled task) — per-device
                    // prefs are irrelevant, treat as already configured
                    continue;
                }
            } else {
                PreferenceStore::get(&self.pool, key).await
            };
            let value = match read {
                Ok(v) => v,
                Err(e) => {
                    log!(
                        "[Chat] preference read failed for '{}': {}. Treating as configured so the turn is not refused",
                        key,
                        e
                    );
                    continue;
                }
            };
            if value.is_none() {
                missing_instructions.push(*instruction);
                missing_pref_keys.push(*key);
            }
        }

        let language_section = if !missing_instructions.is_empty() {
            log!(
                "[Chat] Setup required — missing preferences: {}",
                missing_instructions.join(", ")
            );
            format!("SETUP REQUIRED — DO NOT PROCEED UNTIL COMPLETE:\nThe following settings are not configured. You MUST ask the user for these BEFORE doing anything else. Do NOT answer questions, create tasks, or perform any work until setup is complete.\n{}", missing_instructions.join("\n"))
        } else {
            format!(
                "USER LANGUAGE: {}\nAlways respond in {}.",
                user_language, user_language
            )
        };

        let system_prompt = workspace_identity_section(
            &self.workspace_name(),
            &self.workspace_path,
            &timezone_section,
            &language_section,
        );

        // The one divergence between the two prompt variants: whether this
        // install has Lucidos platform source to edit. Same signal as the
        // `/health` `packaged` flag the compose picker gates on.
        let has_lucidos_source = crate::paths::has_lucidos_source();

        let system_prompt = format!(
            "{}{}",
            system_prompt,
            static_prompt_body(has_lucidos_source, max_tool_calls)
        );

        // Whether the user has restarted onto the newest build is a FACT the
        // engine holds, so it is stated here rather than left to be inferred
        // from having applied a `requires_restart` change earlier in the
        // thread. On 2026-08-09 that inference produced "the engine is still
        // serving the pre-restart build" about an engine that had been
        // restarted, and the agent had no way to check: the rule's own
        // prescribed probe (fetch a served asset) says nothing about a change
        // that altered no served asset, which a backend-only change never
        // does. Dev-only, on the same gate as the apply-verify addendum that
        // reads it: a packaged install has no source rebuild and routes to the
        // release updater instead.
        let system_prompt = if has_lucidos_source {
            format!(
                "{}{}",
                system_prompt,
                engine_build_section(&self.version_status().await)
            )
        } else {
            system_prompt
        };

        // Tell the LLM where Lucidos is running
        let api_port = std::env::var("LUCIDOS_API_PORT").unwrap_or_else(|_| "3000".to_string());
        let frontend_url = if let Some(origin) = self.frontend_origin.lock().unwrap().as_ref() {
            origin.clone()
        } else {
            // Fallback when no request origin has been observed yet: the engine
            // serves the frontend itself, so its own TLS setting decides the
            // scheme (never hardcode http/https — see the intra-host scheme rule).
            let scheme = crate::net_config::tls_scheme();
            let port = std::env::var("VITE_PORT").unwrap_or_else(|_| api_port.clone());
            format!("{}://localhost:{}", scheme, port)
        };
        let system_prompt = format!("{}\n\nThe Lucidos client the user is talking to you from is at {}. To see an app UI, use capture_app, never browser_open.",
            system_prompt, frontend_url);

        // NO auto-detected browser-login domain list here, deliberately. The
        // `browser_logins` table is populated by `runtime::browser::session`
        // from whatever the user's persistent browser profile happens to hold
        // a session for — unfiltered browsing data, including work/employer
        // sites. Pasting it into the prompt shipped that list to the model
        // provider on EVERY turn, and nothing consumed it: the BROWSER TOOLS
        // section already tells the agent to retry with `visible=true` when a
        // site redirects to a login page, and `browser_forget_login` takes an
        // explicit domain from the user. The table and both browser tools
        // stay; only this prompt section is gone. See
        // `.claude/rules/no-private-data.md`.

        // Add available apps to system prompt. The section is built in
        // `workspace_payload`, which caps each user-authored description: this
        // is the same workspace payload the file list belongs to, not engine
        // prose.
        let apps_section = self
            .app_manager
            .list_apps()
            .map(|apps| super::workspace_payload::build_apps_section(&apps))
            .unwrap_or_default();

        // Add available prompts to system prompt (app-scoped + standalone triggers)
        let data_dir = self.workspace_path.join(crate::core::DATA_DIR);
        let all_intents = crate::core::IntentStore::load_all(&data_dir);
        let intents_section = if !all_intents.is_empty() {
            let mut section = String::from("\n\n## Available Intents\n\n");
            section.push_str("Stored descriptions of what the user wants. Use execute_intent(intent_id) to fulfill one. Each intent is paired with knowhow that tells you how to achieve it.\n\n");
            for p in &all_intents {
                section.push_str(&format!("- **{}** (id: `{}`)\n", p.name, p.id));
            }
            section
        } else {
            String::new()
        };

        // Add know-how summaries (general + app-specific)
        let kh_dirs = self.knowhow_dirs();
        let knowhow_summaries = crate::core::KnowhowStore::load_merged_summaries(&kh_dirs);
        let apps_dir = self.workspace_path.join(crate::core::APPS_DIR);
        let app_knowhow_summaries = crate::core::KnowhowStore::load_app_summaries(&apps_dir);
        let knowhow_section = super::workspace_payload::build_knowhow_section(
            &knowhow_summaries,
            &app_knowhow_summaries,
        );

        // Add system knowhow summaries (engine-shipped reference, never overrideable).
        let system_knowhow_summaries = self
            .system_knowhow_dir()
            .map(crate::core::SystemKnowhowStore::load_summaries)
            .unwrap_or_default();
        let system_knowhow_section = build_system_knowhow_section(&system_knowhow_summaries);

        let system_prompt = format!(
            "{}{}{}{}{}",
            system_prompt, apps_section, intents_section, knowhow_section, system_knowhow_section
        );

        let image_provider_available = self.current_image_provider().await.is_some();

        let system_prompt = {
            let mut section = format!("{}\n\n## Images\n\n\
                Images in the conversation are numbered sequentially (1-based) across all messages — user-pasted and generated. \
                The conversation history notes which messages had images with their thread:N index \
                (e.g. \"[attached image (thread:2)]\"). When images are included in the message content, \
                they are labeled as \"from earlier in the conversation\" or \"attached to current message\" \
                so you can tell which are new. \
                Older images age out of your vision after a few messages — the history then shows only a \
                text note like \"[attached image (thread:2) — image not included]\" plus a description. \
                When the user refers to an image you can no longer see, call the view_image tool with its \
                reference (e.g. image: 'thread:2') to load it back into your vision, then answer from what \
                you see — do NOT claim you have no image or ask the user to re-send it. \
                You can save any conversation image to an artifact file with the save_thread_image tool \
                (e.g., image: 'thread:1', path: 'artifacts/photos/reaction.jpg').", system_prompt);
            if image_provider_available {
                section.push_str(" You can also generate or edit images with the generate_image tool. \
                    To edit an existing image, reference it as 'thread:N' where N is its position in the thread, \
                    or use an artifact path like 'artifacts/photo.png'. \
                    When the user says \"edit the second image\", use input_images: [\"thread:2\"].");
            }
            section
        };

        // Trigger-fire framing rules (static — see TRIGGER_SYSTEM_ADDENDUM
        // docs). Appended last so the unconditional sections above stay
        // byte-identical between trigger fires and regular chats — that
        // shared prefix is what the LLM provider's prompt cache keys on.
        // The per-trigger knowhow listing is also trigger-only, so it lives
        // here next to the addendum (NOT in the unconditional knowhow_section
        // above, which would invalidate the cache prefix for every other
        // trigger thread).
        let system_prompt = if is_trigger {
            let trigger_knowhow_section = trigger
                .as_ref()
                .map(|t| {
                    let triggers_dir = self.workspace_path.join(crate::core::TRIGGERS_DIR);
                    build_trigger_knowhow_section(&triggers_dir, &t.slug)
                })
                .unwrap_or_default();
            format!(
                "{}{}{}",
                system_prompt,
                trigger_knowhow_section,
                crate::scheduler::user_tasks::TRIGGER_SYSTEM_ADDENDUM
            )
        } else {
            system_prompt
        };

        (system_prompt, missing_pref_keys, image_provider_available)
    }
}

/// Build the prompt's opening identity block: who the agent is, which
/// workspace it runs in, the timezone section, the language section, and the
/// personal-data-access framing. Pure so the workspace line's path rendering
/// is unit-testable without standing up a `LucidosEngine`.
///
/// The workspace path is rendered through [`crate::core::home_path::abbreviate`]:
/// on an MDM-managed fleet the home dir is named `<username>@<employer-domain>`,
/// so the raw absolute path shipped the user's employer to the model provider
/// on every turn. `~/…` keeps the path's shape without the identity. The
/// engine's own path handling is unaffected — this is display text only.
fn workspace_identity_section(
    workspace_name: &str,
    workspace_path: &Path,
    timezone_section: &str,
    language_section: &str,
) -> String {
    format!(
        r#"You are managing Lucidos, a personal assistant running in the "{workspace_name}" workspace. You help users organize their life and work through natural conversation.

WORKSPACE: {workspace_name} ({workspace_path})
All threads, events, artifacts, and data you access belong to this workspace. When the user refers to "my threads", "events", or other data, it means data in this workspace.

{timezone_section}

{language_section}

PERSONAL DATA ACCESS:
This is the user's PRIVATE workspace containing THEIR OWN personal documents, files, and data.
The user has FULL rights to access, view, and discuss ANY information in their workspace.
This includes personal identifiers (SSN, ID numbers, addresses, phone numbers, etc.) from their own documents.
When the user asks about content in their files, provide it - this is their data, not a privacy violation.
Do NOT refuse to discuss the user's own personal information from their own files."#,
        workspace_name = workspace_name,
        workspace_path = crate::core::home_path::abbreviate(workspace_path),
        timezone_section = timezone_section,
        language_section = language_section,
    )
}

#[cfg(test)]
mod tests {
    use super::super::super::process_helpers::{
        APPLY_VERIFY_DEV_ADDENDUM, APPLY_VERIFY_RULE, LOOK_BEFORE_ASSESSING_RULE,
    };
    use super::{
        coding_surface_section, engine_build_section, static_prompt_body,
        workspace_identity_section, ASK_USER_QUESTION_RULE, NAMES_NOT_IDS_RULE,
        NO_IMPERSONATION_RULE, SETUP_INTERVIEW_RULE, TRIGGER_VS_EVENT_WAIT_RULE,
        WORKSPACE_ASSETS_KNOWHOW_RULE,
    };
    use crate::engine::engine_version::VersionStatus;
    use std::path::{Path, PathBuf};

    /// Ceiling on the engine-authored text every chat turn pays for before the
    /// user has typed a word: the static prompt body with its rule consts
    /// substituted, every tool schema the chat agent is offered in its wire
    /// form, and the System Knowhow routing list built from frontmatter
    /// `description:` lines.
    ///
    /// A RATCHET, not a target. Set just above where the 2026-08-07 trim landed
    /// (see `docs/plans/2026-08-07-always-loaded-prompt-budget-trim.md`). A new
    /// rule, tool or knowhow file has to pay for itself by trimming elsewhere,
    /// or this number moves in a change that says why it is worth it. Every
    /// character here is billed on every request of every thread in every
    /// workspace, so growth is never free.
    ///
    /// Raised from 133,000 the same day for `list_event_waits` and
    /// `cancel_event_wait` (ADR 0052). What the ~400 net characters buy is the
    /// agent being able to tell the truth about its own subscriptions: before
    /// them it could arm a watch and then neither read nor revoke it, so "yes,
    /// still watching" was a guess (wrong by two hours on 2026-08-06) and "I'll
    /// stop watching" was unenforceable. Both descriptions were cut to about
    /// half their first draft, and `await_event`'s own pointer to them to a
    /// sentence, before this number moved.
    ///
    /// LOWERED from 133,500 by the 2026-08-07 tool-schema trim
    /// (`docs/plans/2026-08-07-tool-schema-budget-trim.md`), which cut the two
    /// schema areas the earlier pass had left almost untouched, plus the three
    /// restart / apply-verify rule consts it had reported as headroom:
    ///
    /// | Area | Before | After |
    /// |---|---|---|
    /// | static prompt body + rule consts | 32,438 | 31,777 |
    /// | flat tool schemas | 65,897 | 49,692 |
    /// | grouped manifest tool schemas | 23,940 | 18,619 |
    /// | System Knowhow routing list | 11,130 | 11,261 |
    ///
    /// No rule was deleted. What went was duplication: a schema restating a
    /// policy the static prompt already owns keeps a one-clause pointer, and
    /// detail that is needed but not needed every turn moved into the
    /// `system-knowhow/*.md` body the schema now names, which is retrieved
    /// rather than resident. `tool_summary` shed the action enumeration
    /// `build_llm_tool` prints on the very next lines.
    ///
    /// The one area that GREW is the routing list, by 131 chars, and it grew
    /// because a doc did: `coding-agent-events` gained the `run_coding_agent`
    /// folder table, so its frontmatter `description` had to say so or the
    /// content would be unroutable. A description is the routing signal, not a
    /// summary, so it tracks coverage.
    ///
    /// Paired with `no_single_tool_schema_dominates_the_always_loaded_budget`,
    /// because this number alone lets one runaway schema hide behind twenty
    /// lean ones.
    ///
    /// Set at 111,200 when the trim first landed, then moved to 111,400 in the
    /// same change after hardening restored four things the trim had dropped:
    /// the ask-before-persisting rule and its knowhow pointer on `git_clone`,
    /// the review-the-result instruction on `run_thread`, the real
    /// `context_window` guess table (the trim had collapsed it to "200k", wrong
    /// for the two ids that guess higher), and the fact that a password
    /// credential injects no bare `CRED_<NAME>`. Roughly half of those 190
    /// characters were paid for out of manifest slack rather than the ceiling.
    ///
    /// Ratcheted to 103,200 once two further passes landed and were measured
    /// together on main: the System Knowhow routing list from 11,261 to 8,453
    /// chars, by rewriting descriptions as coverage plus verbatim trigger
    /// phrases, and the static body from 31,777 to 29,037 with the per-tool
    /// ceiling tightened to 1,500. Measured total is 102,998, so this leaves
    /// 202 characters of headroom. The ratchet is deliberately tight: a pass
    /// that reclaims space lowers this line in the same change, or the space
    /// it reclaimed is silently spent by the next thing that grows.
    ///
    /// Raised to 103,650 for the *trigger model*: the grouped `triggers` tool
    /// gained `model` and `reasoning_effort` on create/update, 408 characters
    /// of which 202 is frozen shape (the null-union on both, and the six-value
    /// effort enum). The capability is not UI-only by choice: "run that digest
    /// on a cheap model" has to work from the prompt (`.claude/rules/philosophy.md`
    /// rule 2, prompt-first), and the schema is the only surface that tells the
    /// agent the parameter exists. Nothing was trimmed to pay for it because
    /// the base measured 103,195 against the 103,200 line, i.e. there was no
    /// slack left to reclaim. Measured total is 103,603, leaving 47 characters.
    ///
    /// Raised to 103,950 for two sentences a real thread paid for on
    /// 2026-08-09. It spent five hours idle after ending a turn with "I have a
    /// watch loop draining both legs": a background task outlives the turn, but
    /// nothing re-opens the thread when it finishes, and only a user message
    /// eventually did. It then completed the work and never called `todo_write`
    /// again, so its finished plan still reads `abandoned`.
    ///
    /// Both are facts about the ENGINE, which the model cannot infer, and both
    /// are decided at the END of a turn, which is exactly when nothing is being
    /// looked up: `running-python` already carries the `await_event on
    /// BackgroundBashCompleted` recipe in full and that thread never loaded it.
    /// That is what makes these two resident and the recipe knowhow.
    ///
    /// Placement follows the same reasoning. `run_bash_background` (+227)
    /// rather than `bash_output`, because the plan to follow a task is formed
    /// when it is spawned, and because `bash_output` sits 14 chars under the
    /// per-tool ceiling, which is that test correctly refusing prose the
    /// knowhow already owns. `todo_write` (+82) rather than `await_event`,
    /// which is the tool a thread in this state is precisely NOT reaching for.
    /// Measured total is 103,901, leaving 49 characters.
    ///
    /// **That entry's wake claim is now out of date, and the sentence with it.**
    /// It said a chat or trigger thread has no `agent_sessions` entry for
    /// `spawn_bash_completion_watcher` to push a resume prompt into, so nothing
    /// re-opens it. True when written; the chat turn tail now arms an *event
    /// wait* over any unwatched background task (`engine::event_wait::background_task`),
    /// so the engine does re-open the thread. The schema says that instead, in
    /// 60 fewer characters, and instructing the model to arm the wait itself
    /// would now be telling it to do what has already been done.
    ///
    /// **Raised to 105,900 in one step by the merge that brought all of the
    /// above together**, and stated as one delta on purpose: this branch and
    /// `main` each raised the line from the same 103,603 base, so their
    /// individual arithmetic (103,950 there, 104,180 here) no longer composes,
    /// and a ladder of deltas that does not add up to the measurement is worse
    /// than no ladder. What the +1,949 over 103,950 buys, measured on the
    /// merged tree:
    ///
    /// - The **ENGINE BUILD section**, 371, billed at its longest of three
    ///   states even though a turn renders one. It is metered at all only
    ///   because this work added it to `always_loaded_areas`: it is built per
    ///   turn rather than spliced into `static_prompt_body`, so it would
    ///   otherwise have been always-loaded text outside the only meter that
    ///   watches always-loaded text. It buys the restart question, which the
    ///   prompt was previously paying for in wrong answers: a thread asserted
    ///   the user had not restarted when they had, and the prescribed probe
    ///   cannot see a backend-only change, so there was no way to check.
    /// - The **lookup surface**, 1,369 across three grouped schemas
    ///   (`memory` gained `search` and `source`, `threads` gained `search`,
    ///   `events` gained a `thread_id` filter) plus 590 for
    ///   `LOOK_BEFORE_ASSESSING_RULE` in the static body. They close the gap
    ///   that let a thread assert the user's launch had not happened while the
    ///   facts sat in memory: retrieval ran once, before the turn, from queries
    ///   a classifier guessed, and nothing could ask again. The root fix is
    ///   free (a decomposition rule inside `QUERY_CLASSIFICATION_PROMPT`, which
    ///   is not resident); this is the backstop, and a backstop nobody can
    ///   reach is not one.
    /// - Minus 60, because the `run_bash_background` sentence above was
    ///   rewritten rather than added to: the engine now arms that wait itself.
    ///
    /// Paid down first by stripping the when-to-search policy out of all three
    /// schemas, which the rule owns and which is the duplication the per-tool
    /// ceiling exists to catch: the three landed 1,171 characters below their
    /// first draft. All three still exceed the shared 1,500 per-tool line and
    /// are enumerated in `PER_TOOL_CEILING_EXCEPTIONS` with the structure
    /// behind each. Measured total is 105,845, leaving 55 characters.
    ///
    /// Raised to 106,150 for `cancel_event_wait`'s third target, `on`: stand
    /// down the subscriptions watching one event type. 245 characters, and
    /// what they buy is the SAFE verb for the request the agent actually gets.
    /// "Stop watching for the release build" had only two forms, and both are
    /// bad: read the list, find the id, then cancel it (three steps, and the
    /// id is a uuid the model has to carry between calls), or reach for `all`,
    /// which silently ends watches nobody mentioned. That is the harm ADR 0052
    /// exists to prevent, reachable in one hasty tool call, and an agent that
    /// skips the list to save a step lands on it. The argument is also on the
    /// CLI, where the caller cannot read a list at all: `scripts/lib/e2e_lock.sh`
    /// stands down the acquiring thread's watch for `E2ELockReleased` the moment
    /// it takes the lock. Both surfaces share one engine function, so leaving
    /// `on` off the schema would mean the refusals naming it were addressed to
    /// an argument only one caller had. Measured total is 106,090, leaving 60.
    const ALWAYS_LOADED_BUDGET_CHARS: usize = 106_150;

    /// The hand-written flat tool schemas the chat agent is offered.
    ///
    /// Mirrors the tool splice in `chat::process::run`, minus the two parts
    /// that are not engine-authored text: tools discovered from running MCP
    /// servers, and `generate_image` (present only when an image provider is
    /// configured). The grouped manifest tools are billed separately because
    /// they have a different owner, `crate::capability_manifest`.
    fn flat_chat_tools() -> Vec<crate::llm::provider::ToolDefinition> {
        let mut flat = crate::llm::tools::get_default_tools();
        flat.push(crate::llm::tools::get_notification_tool());
        flat.push(crate::llm::get_navigate_ui_tool());
        flat.push(crate::llm::tools::get_save_thread_image_tool());
        flat.push(crate::llm::tools::get_view_image_tool());
        flat
    }

    /// Characters a tool schema costs on the wire, as the provider sees it.
    fn wire_chars(tool: &crate::llm::provider::ToolDefinition) -> usize {
        serde_json::to_string(tool)
            .expect("a tool schema serializes")
            .chars()
            .count()
    }

    /// The four always-loaded areas, measured separately so a regression report
    /// says WHICH one grew.
    fn always_loaded_areas() -> Vec<(&'static str, usize)> {
        // The dev variant is the larger prompt on one axis (it appends
        // APPLY_VERIFY_DEV_ADDENDUM) and the smaller on another (its
        // coding-surface section is shorter), so measure both and bill the
        // worse case.
        let body = std::cmp::max(
            static_prompt_body(true, 500).chars().count(),
            static_prompt_body(false, 500).chars().count(),
        );

        let flat: usize = flat_chat_tools().iter().map(wire_chars).sum();
        let grouped: usize = crate::capability_manifest::llm_tools()
            .iter()
            .map(wire_chars)
            .sum();

        let repo = crate::paths::repo_root().expect("repo root resolves under cargo test");
        let knowhow = super::super::super::process_helpers::build_system_knowhow_section(
            &crate::core::SystemKnowhowStore::load_summaries(&repo.join("system-knowhow")),
        )
        .chars()
        .count();

        // Billed at its WORST case, like the body above. The section is built
        // per turn rather than spliced into `static_prompt_body`, so it would
        // otherwise be always-loaded text sitting outside the only meter that
        // watches always-loaded text, which is exactly how an unbudgeted
        // surface grows. Dev-only, and the dev variant is the one measured.
        let engine_build = [(false, false), (true, false), (false, true)]
            .into_iter()
            .map(|(update_available, source_behind_head)| {
                engine_build_section(&version_status(update_available, source_behind_head))
                    .chars()
                    .count()
            })
            .max()
            .expect("three states");

        vec![
            ("static prompt body + rule consts", body),
            ("flat tool schemas, JSON wire form", flat),
            ("grouped manifest tool schemas, JSON wire form", grouped),
            ("System Knowhow routing list", knowhow),
            ("ENGINE BUILD section (dev, per turn)", engine_build),
        ]
    }

    /// The `n` costliest tool schemas, so a breach names the tool that grew
    /// rather than only the area it sits in.
    fn largest_tool_schemas(n: usize) -> String {
        let mut tools = flat_chat_tools();
        tools.extend(crate::capability_manifest::llm_tools());

        let mut rows: Vec<(usize, String)> = tools
            .iter()
            .map(|t| (wire_chars(t), t.name.clone()))
            .collect();
        rows.sort_by(|a, b| b.0.cmp(&a.0));
        rows.truncate(n);
        rows.iter()
            .map(|(chars, name)| format!("  {chars:>7} chars  {name}"))
            .collect::<Vec<_>>()
            .join("\n")
    }

    /// Every tool in one area, costliest first, with its area total. The
    /// per-tool ceiling test and the budget test both print a truncated
    /// ranking; this is the whole thing, for a trim that has to see the tail.
    fn full_ranking(label: &str, tools: &[crate::llm::provider::ToolDefinition]) -> String {
        let mut rows: Vec<(usize, &str)> = tools
            .iter()
            .map(|t| (wire_chars(t), t.name.as_str()))
            .collect();
        rows.sort_by(|a, b| b.0.cmp(&a.0));
        let total: usize = rows.iter().map(|(n, _)| n).sum();
        let body = rows
            .iter()
            .map(|(chars, name)| format!("  {chars:>7} chars  {name}"))
            .collect::<Vec<_>>()
            .join("\n");
        format!("{label} ({} tools, {total} chars):\n{body}", rows.len())
    }

    /// Ceiling on ONE tool schema's JSON wire form, flat or grouped.
    ///
    /// `ALWAYS_LOADED_BUDGET_CHARS` is a single total, which lets one runaway
    /// schema hide behind twenty lean ones: the 2026-08-07 trim found
    /// `run_coding_agent` at 5,207 characters while the total was comfortably
    /// under its ceiling. Same reasoning as
    /// `system_knowhow_descriptions_stay_routing_sized`, which caps each
    /// frontmatter `description` rather than their sum.
    ///
    /// 1,500 is where the tool set sits after the 2026-08-07 third-pass trim:
    /// 64 of the 71 tools clear it, most of them by a wide margin. The seven
    /// that do not are enumerated in [`PER_TOOL_CEILING_EXCEPTIONS`] with a
    /// reason each, so an eighth is a deliberate act rather than a drift.
    ///
    /// Lowered from 2,000 by that pass. The number is chosen against the
    /// FROZEN CONTRACT rather than picked round: strip every `description` from
    /// a schema (what `print_frozen_tool_contract` dumps) and what remains is
    /// the part a trim may not touch. For all but the seven that floor is a few
    /// hundred characters, so 1,500 leaves room for prose without inviting any.
    const PER_TOOL_SCHEMA_CEILING_CHARS: usize = 1_500;

    /// The tools allowed past [`PER_TOOL_SCHEMA_CEILING_CHARS`], each with the
    /// reason its schema is structurally larger than the rest.
    ///
    /// Every value is a RATCHET set just above where the trim landed, so
    /// growing one of these is as deliberate as adding a row. A reason that is
    /// only "it is long" does not belong here: the test is what stops a schema
    /// re-accumulating the prose the prompt and the knowhow already own.
    ///
    /// Each reason names the STRUCTURE that cannot shrink, and the frozen-
    /// contract floor is the arithmetic behind it: `navigate_ui` 751 chars,
    /// `triggers` 744, `request_credential` 571, `run_coding_agent` 474,
    /// `ask_user_question` 434, `await_event` 337, `follow_up_child_thread`
    /// 175. Where a row's ceiling is far above its floor, the gap is prose two
    /// or more tests pin phrase by phrase, which is enumerated in the reason.
    const PER_TOOL_CEILING_EXCEPTIONS: &[(&str, usize, &str)] = &[
        (
            "await_event",
            2_550,
            "four tests pin thirteen distinct phrases on it (forward-only plus \
             the arming race, spent-and-resubscribe, the consecutive cap, the \
             own-child carve-out), each one a failure that reached a user, and \
             `condition` carries the operator set that makes a filter valid",
        ),
        (
            "run_coding_agent",
            2_600,
            "eleven parameters; the source-checkout precondition is pinned in \
             BOTH the description and `folder` because a packaged install's \
             agent believed the unconditional wording and narrated a spawn the \
             engine refuses, and the spawn-ack-is-not-a-result rule stays \
             inline rather than behind the knowhow pointer because it fires at \
             spawn time, before anything would be loaded",
        ),
        (
            "triggers",
            2_950,
            "seven actions, each contributing its own summary line and its own \
             `(requires: …)` clause, plus the create schema's union shapes for \
             `cron` and `on`. system-knowhow/triggers.md deliberately does NOT \
             restate the cron format, it defers to this schema, so the format \
             spec cannot move out. Raised from 2,550 by the *trigger model*: \
             `model` and `reasoning_effort` add 202 characters of frozen shape \
             before a word of prose (a null-union each, and the six-value \
             effort enum), and they are declared once, on create, because the \
             union across operations is first-wins",
        ),
        (
            "navigate_ui",
            2_300,
            "the two frozen enums the SDK is generated from (17 targets, 17 \
             settings views) are 751 chars before a word of prose; the settings \
             gloss is routing information available on no other surface",
        ),
        (
            "request_credential",
            2_150,
            "thirteen properties, seven of them the oauth_client endpoint set, \
             each already one line pointing at system-knowhow/oauth-providers",
        ),
        (
            "ask_user_question",
            1_950,
            "the \"Other\"-option ban is deliberately mirrored between \
             ASK_USER_QUESTION_RULE and this schema, and pinned by a test in \
             each place; the nested question / options / label object is 434 \
             chars of frozen shape",
        ),
        (
            "follow_up_child_thread",
            1_900,
            "five phrases pinned by three tests, every one a side effect that \
             is invisible from the verb",
        ),
        (
            "memory",
            1_780,
            "four actions where there were two: `search` and `source` turned \
             this from a correction-only tool into a read, each contributing \
             its own summary line, its own `(requires: …)` clause and its own \
             argument schema. The discipline on when to search is NOT here, \
             deliberately: LOOK_BEFORE_ASSESSING_RULE owns it, so the schema \
             says what the action does and nothing about policy",
        ),
        (
            "threads",
            1_860,
            "a third action on a domain whose two existing schemas are almost \
             entirely a spelled-out `status` enum pinned to `ThreadStatus::ALL` \
             by a test, repeated across both because `llm_schema` is a const \
             JSON literal and cannot compose. `search` adds its own summary, \
             `(requires: q)` and two properties on top of that frozen shape",
        ),
        (
            "events",
            1_650,
            "one property added to a domain already at 1,485: `thread_id` is \
             the read half of finding a past conversation, and it has to say \
             which `event_type` to pair it with or the filter returns the \
             whole transcript including every streamed token",
        ),
    ];

    /// Caps ONE tool schema, so a single runaway cannot hide inside a total
    /// that still passes. See [`PER_TOOL_SCHEMA_CEILING_CHARS`].
    #[test]
    fn no_single_tool_schema_dominates_the_always_loaded_budget() {
        let mut tools = flat_chat_tools();
        tools.extend(crate::capability_manifest::llm_tools());

        let mut breaches = Vec::new();
        for tool in &tools {
            let chars = wire_chars(tool);
            let (ceiling, reason) = PER_TOOL_CEILING_EXCEPTIONS
                .iter()
                .find(|(name, _, _)| *name == tool.name)
                .map(|(_, ceiling, reason)| (*ceiling, Some(*reason)))
                .unwrap_or((PER_TOOL_SCHEMA_CEILING_CHARS, None));
            if chars > ceiling {
                breaches.push(match reason {
                    Some(reason) => format!(
                        "  {chars:>5} chars  {} (over its {ceiling} exception, granted for: {reason})",
                        tool.name
                    ),
                    None => format!(
                        "  {chars:>5} chars  {} (over the {ceiling} shared ceiling)",
                        tool.name
                    ),
                });
            }
        }

        // An exception whose tool has since been renamed or deleted silently
        // stops covering anything, and an exception the tool no longer needs
        // is a licence nobody is using. Both are drift, so both fail here.
        for (name, ceiling, _) in PER_TOOL_CEILING_EXCEPTIONS {
            let Some(tool) = tools.iter().find(|t| t.name == *name) else {
                panic!(
                    "PER_TOOL_CEILING_EXCEPTIONS names `{name}`, which is not a \
                     tool the chat agent is offered. Drop the row, or fix the name."
                );
            };
            assert!(
                wire_chars(tool) > PER_TOOL_SCHEMA_CEILING_CHARS,
                "`{name}` is now {} chars, under the {PER_TOOL_SCHEMA_CEILING_CHARS} \
                 shared ceiling, so its {ceiling} exception is dead. Delete the row.",
                wire_chars(tool)
            );
        }

        assert!(
            breaches.is_empty(),
            "tool schema(s) over their ceiling. A description states what the \
             tool does, when to reach for it rather than a sibling, and its \
             non-obvious failure modes; policy the system prompt owns stays in \
             the prompt, and detail underneath moves into the knowhow the \
             schema points at:\n{}\n\nfull ranking:\n{}\n\n{}",
            breaches.join("\n"),
            full_ranking("flat tool schemas", &flat_chat_tools()),
            full_ranking(
                "grouped manifest tool schemas",
                &crate::capability_manifest::llm_tools()
            ),
        );
    }

    /// Diagnostic dump, not an assertion: every tool schema ranked by wire
    /// cost, both areas, no truncation.
    ///
    /// `always_loaded_context_stays_under_budget` prints the top ten, which is
    /// the right size for spotting a regression but not for planning a trim:
    /// twenty 1,200-char schemas are another 24k and none of them ever reaches
    /// that list. Run before and after a trim to record per-tool deltas.
    ///
    ///   cargo test -p lucidos-engine --lib print_full_tool_schema_ranking -- --ignored --nocapture
    #[test]
    #[ignore]
    fn print_full_tool_schema_ranking() {
        println!(
            "{}\n\n{}",
            full_ranking("flat tool schemas", &flat_chat_tools()),
            full_ranking(
                "grouped manifest tool schemas",
                &crate::capability_manifest::llm_tools()
            ),
        );
    }

    /// Diagnostic dump, not an assertion: every tool's name and `parameters`
    /// JSON with all `description` keys stripped, recursively.
    ///
    /// What survives the strip IS the callable contract: property names, types,
    /// enum values, `oneOf` / `anyOf` branches, `minItems` / `maxItems`, nesting
    /// and `required` sets. A prose-only trim must leave this output
    /// byte-identical, which makes "did I change what the model is allowed to
    /// call?" a diff rather than a judgment. The tool `description` is excluded
    /// for the same reason the per-property ones are: it is the prose under
    /// trim.
    ///
    ///   cargo test -p lucidos-engine --lib print_frozen_tool_contract -- --ignored --nocapture
    #[test]
    #[ignore]
    fn print_frozen_tool_contract() {
        /// Drop every `description` key, at every depth, in a stable order.
        fn strip(value: &serde_json::Value) -> serde_json::Value {
            match value {
                serde_json::Value::Object(map) => serde_json::Value::Object(
                    map.iter()
                        .filter(|(k, _)| k.as_str() != "description")
                        .map(|(k, v)| (k.clone(), strip(v)))
                        .collect(),
                ),
                serde_json::Value::Array(items) => {
                    serde_json::Value::Array(items.iter().map(strip).collect())
                }
                other => other.clone(),
            }
        }

        let mut tools = flat_chat_tools();
        tools.extend(crate::capability_manifest::llm_tools());
        // Sorted so a splice-order change is visible as a reorder rather than
        // as a whole-file diff.
        tools.sort_by(|a, b| a.name.cmp(&b.name));

        for tool in &tools {
            println!(
                "{}\t{}",
                tool.name,
                serde_json::to_string(&strip(&tool.parameters)).expect("schema serializes")
            );
        }
    }

    /// Pins the win from the 2026-08-07 prompt-budget trim so the next
    /// regression is visible at `cargo test` rather than in a token bill.
    ///
    /// Prints the per-area breakdown on failure AND on success under
    /// `--nocapture`, so re-measuring after a deliberate addition costs one
    /// command rather than a fresh harness.
    #[test]
    fn always_loaded_context_stays_under_budget() {
        let areas = always_loaded_areas();
        let total: usize = areas.iter().map(|(_, n)| n).sum();

        let breakdown = areas
            .iter()
            .map(|(name, n)| format!("  {n:>7} chars  {name}"))
            .collect::<Vec<_>>()
            .join("\n");
        println!(
            "always-loaded context:\n{breakdown}\n  {total:>7} chars  TOTAL\n\nlargest tool schemas:\n{}",
            largest_tool_schemas(10)
        );

        assert!(
            total <= ALWAYS_LOADED_BUDGET_CHARS,
            "always-loaded chat context is {total} chars, over the \
             {ALWAYS_LOADED_BUDGET_CHARS} ceiling. Every character is billed on \
             every request of every thread, so trim somewhere else or raise \
             ALWAYS_LOADED_BUDGET_CHARS in a change that says why:\n{breakdown}\n\n\
             largest tool schemas:\n{}",
            largest_tool_schemas(10)
        );
    }

    /// Every `system-knowhow/<id>` the resident context routes to must resolve
    /// to a real file, or `load_knowhow` answers with the not-found sentinel
    /// and the agent proceeds unguided on exactly the request the pointer
    /// exists to catch.
    ///
    /// The per-rule versions of this
    /// (`workspace_assets_rule_names_only_live_knowhow_ids`,
    /// `setup_interview_rule_routes_to_a_live_knowhow_id`) check a hand-listed
    /// set. This one sweeps everything the model is handed, the whole assembled
    /// prompt body AND every tool schema, so a pointer added by a later trim is
    /// covered the moment it is written. The schemas are in scope because they
    /// route too: the 2026-08-07 trim moved detail out of the prompt by ADDING
    /// pointers there, so `create_app` now names `system-knowhow/building-an-app`.
    ///
    /// Ids are filename-derived, so a knowhow rename is what breaks them; both
    /// `triggers` and `plugins` were renamed from `building-a-*` on 2026-08-02.
    #[test]
    fn every_knowhow_id_the_resident_context_routes_to_resolves() {
        let repo = crate::paths::repo_root().expect("repo root resolves under cargo test");

        let mut haystack = format!(
            "{}{}",
            static_prompt_body(true, 500),
            static_prompt_body(false, 500)
        );
        let mut tools = flat_chat_tools();
        tools.extend(crate::capability_manifest::llm_tools());
        for tool in &tools {
            haystack.push_str(&serde_json::to_string(tool).expect("a tool schema serializes"));
        }

        let pointer =
            regex::Regex::new(r"system-knowhow/([a-z0-9-]+)").expect("static pattern compiles");
        let ids: std::collections::BTreeSet<&str> = pointer
            .captures_iter(&haystack)
            .map(|c| c.get(1).expect("group 1 always matches").as_str())
            .collect();
        assert!(
            !ids.is_empty(),
            "the resident context routes to no knowhow at all, the scan is \
             broken rather than the routes being clean"
        );

        let missing: Vec<&str> = ids
            .iter()
            .copied()
            .filter(|id| {
                !repo
                    .join("system-knowhow")
                    .join(format!("{id}.md"))
                    .exists()
            })
            .collect();
        assert!(
            missing.is_empty(),
            "the resident prompt or a tool schema routes to knowhow id(s) with \
             no backing file, so load_knowhow would return the not-found \
             sentinel: {missing:?}"
        );
    }

    /// Every placeholder token in the prompt template must have a matching
    /// `.replace(...)` in the substitution chain, or the raw token ships to the
    /// model verbatim.
    ///
    /// Nothing else catches this. The tokens are plain text inside a raw string
    /// literal, so a dropped `.replace` compiles clean, passes clippy, and only
    /// shows up as gibberish in the LLM's context. The substitution chain is
    /// also a standing merge-conflict site: two branches that each add a rule
    /// collide on the same line, and resolving by picking one side silently
    /// drops the other's substitution while leaving its token in the template.
    /// That is exactly what the 2026-08-02 merge of the knowhow-rename branch
    /// had to resolve by hand.
    ///
    /// Reads its own source rather than a hand-maintained token list, so a new
    /// rule is covered the moment it is added and there is no second list to
    /// keep in sync.
    #[test]
    fn every_prompt_placeholder_is_substituted() {
        let repo = crate::paths::repo_root().expect("repo root resolves under cargo test");
        let path = repo.join("crates/lucidos-engine/src/engine/chat/process/system_prompt.rs");
        let src = std::fs::read_to_string(&path)
            .unwrap_or_else(|e| panic!("read {}: {e}", path.display()));

        let token = regex::Regex::new(r"__[A-Z][A-Z_]*__").expect("static pattern compiles");
        let tokens: std::collections::BTreeSet<&str> =
            token.find_iter(&src).map(|m| m.as_str()).collect();
        assert!(
            !tokens.is_empty(),
            "found no placeholders at all, the scan is broken rather than the template being clean"
        );

        let missing: Vec<&str> = tokens
            .iter()
            .copied()
            .filter(|t| !src.contains(&format!("\"{t}\"")))
            .collect();
        assert!(
            missing.is_empty(),
            "placeholder(s) in the prompt template with no matching .replace(\"…\", …) \
             in the substitution chain, so the raw token would reach the model: {missing:?}"
        );
    }

    /// A direct child's completion is the one state change nobody has to
    /// subscribe to: the ADR 0011 fan-in re-opens the parent with the child's
    /// result whether or not a wait exists. The section used to list "a coding
    /// agent finishing" as a reason to call `await_event` and say nothing about
    /// the fan-in, so on 2026-08-06 a chat thread subscribed to its own
    /// coding-agent child, spending a subscription and arming a 90-minute
    /// timeout for a wake it was going to get anyway.
    ///
    /// **The carve-out is a redundancy, not a limit, and the section has to say
    /// so in that order.** Worded as a bare "not for a thread you spawned", it
    /// was generalised into "cross-thread waits are unreliable": a live thread
    /// armed a `child_thread_id` wait on someone else's coding-agent session and
    /// warned the user it "may never fire". Matching is workspace-wide (see
    /// `a_wait_matches_a_child_completion_belonging_to_another_thread`), so the
    /// positive claim leads and the exclusion follows it.
    ///
    /// Source-scanned rather than asserted against a const, like
    /// `every_prompt_placeholder_is_substituted` above: this section is plain
    /// text inside [`SYSTEM_PROMPT_BASE`], and extracting it would only move the
    /// same text somewhere else. Scoped to the section itself, not the whole
    /// file, so this test's own prose cannot satisfy or break its assertions.
    #[test]
    fn the_state_change_section_excludes_the_threads_own_child() {
        let repo = crate::paths::repo_root().expect("repo root resolves under cargo test");
        let path = repo.join("crates/lucidos-engine/src/engine/chat/process/system_prompt.rs");
        let src = std::fs::read_to_string(&path)
            .unwrap_or_else(|e| panic!("read {}: {e}", path.display()));

        let heading = "WAITING FOR A STATE CHANGE IN LUCIDOS";
        let from = src
            .find(heading)
            .unwrap_or_else(|| panic!("the prompt no longer has a '{heading}' section"));
        let section = &src[from..];
        // The section is one unbroken run of bullets, so the next blank line
        // ends it.
        let section = &section[..section.find("\n\n").unwrap_or(section.len())];

        assert!(
            section.contains("IT WATCHES THE WHOLE WORKSPACE"),
            "another thread's lifecycle is a first-class wait, and the resident \
             prompt has to say so, or the exclusion below reads as 'a wait on a \
             thread that is not mine might not fire':\n{section}"
        );
        assert!(
            section.contains("NOT YOUR OWN CHILD"),
            "the resident prompt must exclude the caller's own child, or the \
             fan-in is invisible at the moment the model picks a mechanism:\n{section}"
        );
        assert!(
            section.contains("a second wake"),
            "the exclusion's reason must be redundancy, never impossibility:\n{section}"
        );
        assert!(
            !section.contains("coding agent finishing"),
            "a coding agent's finish must not be listed as a reason to subscribe: \
             for the caller's own child that is the redundant case the last bullet \
             forbids:\n{section}"
        );
    }

    /// The reported failure, pinned. The block was scoped to "Before creating"
    /// with a `triggers (action create/update)` row, so "run my digest trigger
    /// now" matched no route, the agent never loaded the trigger knowhow, and
    /// it guessed: it resumed a paused trigger, which only restores the
    /// schedule and runs nothing. Narrowing the trigger row back to
    /// create/update reopens exactly that gap.
    #[test]
    fn workspace_assets_rule_routes_existing_trigger_operations() {
        let rule = WORKSPACE_ASSETS_KNOWHOW_RULE;

        assert!(
            rule.contains("EXISTING trigger"),
            "the rule must fire for an existing trigger, not only a create:\n{rule}"
        );
        assert!(
            rule.contains("off-schedule"),
            "running a trigger off-schedule is the case that had no route:\n{rule}"
        );
        assert!(
            rule.contains("ANY action"),
            "the trigger row must not narrow back to create/update:\n{rule}"
        );
        // The action that does the job. Before it existed the rule said "there
        // is NO run now action"; once that sentence is a lie the agent is back
        // to improvising, so the route must name the real action.
        assert!(
            rule.contains("triggers(action=\"run\")"),
            "the rule must name the action that runs a trigger off-schedule:\n{rule}"
        );
        // The specific wrong guess the incident produced.
        assert!(
            rule.contains("resuming a paused trigger does not run it"),
            "must pre-empt the resume-means-run guess:\n{rule}"
        );
        assert!(
            !rule.contains("action create/update"),
            "the create/update-only phrasing is what caused the miss:\n{rule}"
        );
    }

    /// Every id in the routing block must be a real `system-knowhow/<id>`, or
    /// `load_knowhow` answers with the not-found sentinel and the agent
    /// proceeds unguided. The ids are filename-derived, so a knowhow rename
    /// silently invalidates them; both `triggers` and `plugins` were renamed
    /// from `building-a-*` on 2026-08-02.
    #[test]
    fn workspace_assets_rule_names_only_live_knowhow_ids() {
        let rule = WORKSPACE_ASSETS_KNOWHOW_RULE;
        let repo = crate::paths::repo_root().expect("repo root resolves under cargo test");

        for id in ["triggers", "building-an-app", "building-knowhow", "plugins"] {
            assert!(
                rule.contains(&format!("`system-knowhow/{id}`")),
                "routing block must still name system-knowhow/{id}:\n{rule}"
            );
            assert!(
                repo.join("system-knowhow")
                    .join(format!("{id}.md"))
                    .exists(),
                "routed id system-knowhow/{id} has no backing file, so \
                 load_knowhow would return the not-found sentinel"
            );
        }
        for stale in ["building-a-trigger", "building-a-plugin"] {
            assert!(
                !rule.contains(stale),
                "renamed knowhow id {stale} still routed:\n{rule}"
            );
        }
    }

    /// The setup interview's discovery route must name a knowhow id that
    /// actually resolves. Same failure mode as
    /// `workspace_assets_rule_names_only_live_knowhow_ids`: the id is
    /// filename-derived, so renaming the file leaves the route pointing at
    /// nothing and `load_knowhow` answers with the not-found sentinel, at which
    /// point the button fires and the agent improvises an interview.
    #[test]
    fn setup_interview_rule_routes_to_a_live_knowhow_id() {
        let repo = crate::paths::repo_root().expect("repo root resolves under cargo test");
        assert!(
            SETUP_INTERVIEW_RULE.contains("`system-knowhow/setup-interview`"),
            "the route must name the knowhow id:\n{SETUP_INTERVIEW_RULE}"
        );
        assert!(
            repo.join("system-knowhow/setup-interview.md").exists(),
            "routed id system-knowhow/setup-interview has no backing file, so \
             load_knowhow would return the not-found sentinel"
        );
    }

    /// The route has to survive the ACTION FIRST rule that follows it in the
    /// prompt. That rule says "Don't ask clarifying questions" and excepts only
    /// workspace assets, so without an explicit carve-out the agent reads the
    /// interview request as a normal request, skips the ladder, and builds
    /// something generic on turn one. The carve-out IS the feature here: the
    /// questions are the work.
    #[test]
    fn setup_interview_rule_carves_itself_out_of_action_first() {
        assert!(
            SETUP_INTERVIEW_RULE.contains("ACTION FIRST rule below does NOT apply"),
            "the interview must except itself from ACTION FIRST, or the agent \
             skips the questions:\n{SETUP_INTERVIEW_RULE}"
        );
    }

    /// Cross-layer pin for the entry point's discovery. The welcome CTA and the
    /// header help button send a FIXED sentence as an ordinary user message
    /// (`SETUP_INTERVIEW_PROMPT` in `store/actions/compose.ts`), and the route
    /// only fires if the prompt says something the route recognises. All three
    /// sides carry this phrase verbatim; the frontend half is pinned from its
    /// own direction by `welcome-onboarding.test.tsx`. Reading the sources here
    /// is what makes this a real check on the set rather than three independent
    /// assertions that the same string equals itself.
    ///
    /// The knowhow's frontmatter `description` is the third arm because it is
    /// the SECOND discovery path: retrieval, for a user who types the request
    /// themselves rather than pressing the button. A description that no longer
    /// mentions the sentence leaves the feature alive on the hardcoded route
    /// alone, which is exactly the kind of silent single-point-of-failure this
    /// pair of paths exists to avoid.
    #[test]
    fn setup_interview_route_matches_the_frontend_seeded_prompt() {
        const ANCHOR: &str = "help me get the most out of lucidos";

        assert!(
            SETUP_INTERVIEW_RULE.to_lowercase().contains(ANCHOR),
            "the route must recognise the phrase the button actually sends:\n{SETUP_INTERVIEW_RULE}"
        );

        let repo = crate::paths::repo_root().expect("repo root resolves under cargo test");
        let read = |rel: &str| {
            let path = repo.join(rel);
            std::fs::read_to_string(&path)
                .unwrap_or_else(|e| panic!("read {}: {e}", path.display()))
                .to_lowercase()
        };

        assert!(
            read("crates/lucidos-app/src/store/actions/compose.ts").contains(ANCHOR),
            "the frontend's seeded prompt no longer carries the phrase the \
             system-prompt route keys on, so clicking the button would start an \
             ordinary chat instead of the setup interview"
        );

        let knowhow = read("system-knowhow/setup-interview.md");
        let description = knowhow
            .split("\n---")
            .next()
            .expect("split always yields at least one element");
        assert!(
            description.contains(ANCHOR),
            "the setup-interview knowhow's frontmatter description no longer \
             carries the seeded phrase, so a user who TYPES the request has to \
             rely on the hardcoded route matching instead of retrieval"
        );
    }

    /// The reported failure, pinned. On an install with no source checkout the
    /// prompt must say so outright — the agent had claimed to read
    /// `crates/lucidos-engine/src/core/oauth.rs`, spawned a coding agent, and
    /// told the user to Apply + rebuild, on a packaged install with no source.
    #[test]
    fn no_source_variant_denies_editing_lucidos_and_names_what_works() {
        let section = coding_surface_section(false);

        assert!(
            section.contains("NOT launched from a Lucidos source checkout"),
            "must state the install has no source checkout:\n{section}"
        );
        assert!(
            section.contains("CANNOT edit Lucidos itself"),
            "must deny platform edits outright:\n{section}"
        );
        assert!(
            section.contains("REFUSED"),
            "must warn that the no-`folder` spawn is refused, so the agent \
             doesn't narrate a session it can't start:\n{section}"
        );
        // The two escalations that made this user-visible: inventing source
        // knowledge, and prescribing an apply/rebuild that cannot happen.
        assert!(
            section.contains("NEVER say or imply you have read"),
            "must forbid claiming to have read the source:\n{section}"
        );
        assert!(
            section.contains("NEVER tell the user to Apply, rebuild, or restart"),
            "must forbid the apply/rebuild instruction:\n{section}"
        );
        // A denial with no alternative just invites a retry.
        assert!(
            section.contains("data/apps/<id>") && section.contains("manage_repositories"),
            "must name the app + external-repo routes that still work:\n{section}"
        );
    }

    /// The denial is about THIS install, not about the agent. A cross-workspace
    /// `run_coding_agent(workspace="…")` returns at
    /// `agentic_loop_special_tool.rs:351-367` — before the local source guard —
    /// and the TARGET engine applies its own check, so it stays valid here. An
    /// unqualified "never spawn a coding agent for a Lucidos change" would make
    /// the documented cross-workspace capability unreachable from every packaged
    /// install, which is the one nearby way to over-correct this fix.
    #[test]
    fn no_source_variant_preserves_the_cross_workspace_route() {
        let section = coding_surface_section(false);

        assert!(
            section.contains("CROSS-WORKSPACE IS STILL OPEN"),
            "must keep the cross-workspace route open:\n{section}"
        );
        assert!(
            section.contains("run_coding_agent(workspace="),
            "must show the call shape that still works:\n{section}"
        );
        // The refusal has to be scoped, or the carve-out above contradicts it.
        assert!(
            section.contains("aimed at THIS install") && section.contains("local coding agent"),
            "the refusal must be scoped to local spawns so it doesn't read as \
             a blanket ban:\n{section}"
        );
    }

    /// The dev variant keeps today's behaviour: platform source is editable.
    #[test]
    fn source_variant_says_the_platform_source_is_editable() {
        let section = coding_surface_section(true);

        assert!(
            section.contains("running from a Lucidos SOURCE CHECKOUT"),
            "must state a source checkout exists:\n{section}"
        );
        assert!(
            section.contains("`folder` omitted"),
            "must keep the omit-folder route documented:\n{section}"
        );
        assert!(
            !section.contains("CANNOT edit Lucidos itself"),
            "the dev variant must not carry the denial:\n{section}"
        );
    }

    /// Exactly two variants, and they genuinely differ — a refactor that
    /// collapsed them (or returned the same constant twice) would silently
    /// restore the bug on packaged installs.
    #[test]
    fn the_two_variants_are_distinct() {
        assert_ne!(
            coding_surface_section(true),
            coding_surface_section(false),
            "the source and no-source prompt variants must differ"
        );
    }

    /// The apply/verify split: `changes` guidance is install-independent (app
    /// coding-agent threads still produce changes on a packaged install), while
    /// the engine rebuild/restart choreography is dev-only — a packaged install
    /// never rebuilds from source.
    #[test]
    fn apply_verify_rule_keeps_changes_guidance_and_quarantines_the_rebuild_dance() {
        assert!(
            APPLY_VERIFY_RULE.contains("`changes` tool (action 'list' / 'apply')"),
            "the changes-tool guidance must hold on every install"
        );
        assert!(
            !APPLY_VERIFY_RULE.contains("works on Lucidos constantly"),
            "the dev-only framing must not be in the unconditional rule"
        );
        assert!(
            !APPLY_VERIFY_RULE.contains("did you restart?"),
            "the rebuild/restart dance must not be in the unconditional rule"
        );
        assert!(
            APPLY_VERIFY_DEV_ADDENDUM.contains("works on Lucidos constantly")
                && APPLY_VERIFY_DEV_ADDENDUM.contains("did you restart?"),
            "the dev addendum must carry both dev-only halves"
        );
    }

    /// The failure of 2026-08-09 in one assertion. The addendum used to close
    /// with an example of what to say when the probe showed the old build, and
    /// a thread wrote that sentence verbatim about an engine the user HAD
    /// restarted, having run no probe at all. A prompt that pre-writes a
    /// conclusion invites the model to reach for the words rather than the
    /// check, so no wording for an unverified outcome may live here.
    #[test]
    fn the_addendum_supplies_no_sentence_for_an_unverified_outcome() {
        assert!(
            !APPLY_VERIFY_DEV_ADDENDUM.contains("still serving the pre-restart build"),
            "a ready-made sentence asserting an unverified outcome is what got \
             quoted as a finding:\n{APPLY_VERIFY_DEV_ADDENDUM}"
        );
        assert!(
            APPLY_VERIFY_DEV_ADDENDUM.contains("ENGINE BUILD section"),
            "it must point at where the answer actually is:\n{APPLY_VERIFY_DEV_ADDENDUM}"
        );
    }

    /// The other half of that failure, and the reason pointing at the section
    /// is not enough on its own: the prescribed probe cannot answer for a
    /// backend-only change, because such a change alters no served asset. The
    /// rule has to say so, or a diligent agent still has no method.
    #[test]
    fn the_addendum_scopes_the_probe_to_what_it_can_answer() {
        assert!(
            APPLY_VERIFY_DEV_ADDENDUM.contains("ALTERED a served asset"),
            "the probe must be scoped to changes that alter a served asset:\n\
             {APPLY_VERIFY_DEV_ADDENDUM}"
        );
        assert!(
            APPLY_VERIFY_DEV_ADDENDUM.contains("backend-only change alters none"),
            "it must name the case the probe cannot answer:\n{APPLY_VERIFY_DEV_ADDENDUM}"
        );
    }

    /// The rule must state an ORDER. A rule phrased as a review step fires
    /// once the model has decided what it thinks, which is precisely when it
    /// will not go and check: on 2026-08-09 the agent asserted there had been
    /// no traction, and only looked a turn later after the user objected, by
    /// which point they had supplied the fact themselves.
    #[test]
    fn the_look_rule_states_an_order_not_a_review() {
        assert!(
            LOOK_BEFORE_ASSESSING_RULE.contains("BEFORE YOU FORM AN ASSESSMENT"),
            "the rule must be about ordering:\n{LOOK_BEFORE_ASSESSING_RULE}"
        );
        assert!(
            LOOK_BEFORE_ASSESSING_RULE.contains("not a review at the end"),
            "it must rule out the audit-afterwards reading:\n{LOOK_BEFORE_ASSESSING_RULE}"
        );
    }

    /// The specific claim that went wrong, banned by name. Asserting absence
    /// from an absence of evidence is the one failure this rule exists for.
    #[test]
    fn the_look_rule_bans_asserting_a_negative_unchecked() {
        assert!(
            LOOK_BEFORE_ASSESSING_RULE.contains("NEVER assert something has not happened"),
            "it must ban the unchecked negative:\n{LOOK_BEFORE_ASSESSING_RULE}"
        );
        assert!(
            LOOK_BEFORE_ASSESSING_RULE.contains("Absence from your context is not absence"),
            "it must say WHY, or it reads as a style note:\n{LOOK_BEFORE_ASSESSING_RULE}"
        );
    }

    /// It has to name the two tools, or it is an instruction with no verb: the
    /// agent is told to look and not told what to look with.
    #[test]
    fn the_look_rule_names_the_two_searches_and_bounds_them() {
        assert!(
            LOOK_BEFORE_ASSESSING_RULE
                .contains("`memory` and `threads` tools both have a 'search'"),
            "it must name what to look with:\n{LOOK_BEFORE_ASSESSING_RULE}"
        );
        assert!(
            LOOK_BEFORE_ASSESSING_RULE.contains("not browsing"),
            "the user's constraint (only when necessary) must ride with the \
             permission:\n{LOOK_BEFORE_ASSESSING_RULE}"
        );
    }

    fn version_status(update_available: bool, source_behind_head: bool) -> VersionStatus {
        VersionStatus {
            build_id: "test".to_string(),
            update_available,
            disk_build_id: None,
            packaged: false,
            build_state: "idle",
            source_behind_head,
            shared_build_in_progress: false,
            build_elapsed_ms: None,
            pending_commits: None,
        }
    }

    /// The case the agent got wrong: nothing pending, so an applied
    /// restart-requiring change IS live and the user HAS restarted. Stated
    /// affirmatively, because "no update available" is a double negative the
    /// model has to reason through at exactly the moment it is guessing.
    #[test]
    fn a_current_engine_says_so_affirmatively() {
        let section = engine_build_section(&version_status(false, false));

        assert!(
            section.contains("RUNNING ENGINE IS CURRENT"),
            "must state the current case in the affirmative:\n{section}"
        );
        assert!(
            section.contains("HAS \nrestarted") || section.contains("HAS restarted"),
            "must answer the restart question outright:\n{section}"
        );
    }

    /// A built-but-not-switched engine is the one case where "the user has not
    /// restarted" is true, and it is the only case that may say so.
    #[test]
    fn a_built_but_unswitched_engine_says_the_change_is_not_live() {
        let section = engine_build_section(&version_status(true, false));

        assert!(
            section.contains("HAS NOT SWITCHED ONTO IT YET"),
            "must state that the user has not switched:\n{section}"
        );
        assert!(
            section.contains("is NOT live"),
            "must draw the consequence for an applied change:\n{section}"
        );
    }

    /// Source ahead with no binary behind it is NOT "the user has not
    /// switched": there is nothing to switch onto, so telling them to restart
    /// sends them to a button that would do nothing.
    #[test]
    fn source_ahead_of_a_built_binary_does_not_tell_the_user_to_switch() {
        let section = engine_build_section(&version_status(false, true));

        assert!(
            section.contains("NO BUILD BEHIND IT YET"),
            "must distinguish source-ahead from binary-ready:\n{section}"
        );
        assert!(
            section.contains("nothing to switch onto"),
            "must not send the user to a switch that cannot work:\n{section}"
        );
    }

    /// The three cases must be genuinely different text. Collapsing any two
    /// would restore the guess this section exists to remove.
    #[test]
    fn the_three_build_states_are_distinct() {
        let current = engine_build_section(&version_status(false, false));
        let built = engine_build_section(&version_status(true, false));
        let source_ahead = engine_build_section(&version_status(false, true));

        assert_ne!(current, built);
        assert_ne!(current, source_ahead);
        assert_ne!(built, source_ahead);
    }

    /// It answers the question the addendum forwards to it, and says the
    /// answer can change under the agent's feet: the user restarts on their own
    /// schedule, including while a turn is running.
    #[test]
    fn the_section_answers_the_restart_question_and_dates_itself() {
        let section = engine_build_section(&version_status(false, false));

        assert!(
            section.contains("has the user restarted?"),
            "must name the question it settles:\n{section}"
        );
        assert!(
            section.contains("rebuilt every turn"),
            "must say it is fresh, or a long thread will treat it as stale:\n{section}"
        );
        assert!(
            section.contains("mid-turn"),
            "must warn that the answer can change during a turn:\n{section}"
        );
    }

    /// No build id in the text. The user cannot look one up on any screen, so
    /// putting it here only invites the agent to quote a hex string at them.
    #[test]
    fn the_section_carries_no_build_id() {
        let mut status = version_status(true, false);
        status.build_id = "deadbeef1".to_string();
        status.disk_build_id = Some("cafebabe2".to_string());

        let section = engine_build_section(&status);

        assert!(
            !section.contains("deadbeef1") && !section.contains("cafebabe2"),
            "a build id is meaningless to the user and must not be quotable:\n{section}"
        );
    }

    /// The regression test for the leak this section was built to stop: the
    /// prompt's WORKSPACE line must never carry a `$HOME`-rooted absolute
    /// path. Runs against the REAL `$HOME` (read-only — no env mutation, so it
    /// is safe under the parallel test runner), which is what makes it a true
    /// regression test: on an MDM-managed fleet the home dir is literally
    /// named `<username>@<employer-domain>`, so a regression here fails on the
    /// very machine that would leak.
    #[test]
    fn workspace_line_abbreviates_home_and_leaks_no_absolute_home_path() {
        let home = std::env::var("HOME").expect("HOME must be set to run this test");
        let workspace = PathBuf::from(&home).join("workspaces/myws");

        let section = workspace_identity_section(
            "myws",
            &workspace,
            "CURRENT TIME: now",
            "USER LANGUAGE: English",
        );

        // The workspace is still identified — by name, and by the shape of its
        // path — so the agent loses no usable context.
        assert!(section.contains("WORKSPACE: myws (~/workspaces/myws)"));
        // …but the home dir's own name never reaches the model provider.
        assert!(
            !section.contains(&home),
            "identity section still carries the home-rooted absolute path {home}:\n{section}"
        );

        // The surrounding sections are still spliced in.
        assert!(section.contains("CURRENT TIME: now"));
        assert!(section.contains("USER LANGUAGE: English"));
        assert!(section.contains("PERSONAL DATA ACCESS:"));
    }

    /// Regression guard for the reported card: "Change 99da1708 is docs-only
    /// and its plan is separate from it. What do you want first?", with an
    /// option labelled "Apply 99da1708 now". Nothing the user can see is
    /// labelled with that id, so the question was unanswerable. The rule must
    /// (1) ban raw ids in prose, questions and option labels, (2) say what to
    /// use instead, and (3) keep ids legal in tool arguments.
    #[test]
    fn names_not_ids_rule_bans_raw_ids_in_user_facing_text() {
        let rule = NAMES_NOT_IDS_RULE;
        for needle in [
            "NAMING THINGS TO THE USER, NEVER A RAW ID OR SHA",
            "`ask_user_question` `question` or option label",
            "commit sha",
            "branch name",
        ] {
            assert!(
                rule.contains(needle),
                "rule must ban raw identifiers in user-facing text (`{needle}`):\n{rule}"
            );
        }
        // A ban with no alternative just makes the agent vague.
        assert!(
            rule.contains("thread_title") && rule.contains("its subject line, never the sha"),
            "rule must name the human-readable substitute for each thing:\n{rule}"
        );
        // Presentation-only: the model must keep passing `change_id` to the
        // apply action. Over-correcting here breaks applying, not just wording.
        assert!(
            rule.contains("changes(action='apply', change_id="),
            "rule must keep ids legal in tool arguments:\n{rule}"
        );
        // `run_thread` / `run_coding_agent` results tell the agent verbatim to
        // reply with `[Open thread](thread:<ws>/<uuid>)`. Without this
        // carve-out the rule reads as a ban on that link, and the user loses
        // the only way to open the thread the agent just spawned.
        assert!(
            rule.contains("A MARKDOWN LINK TARGET IS NOT PROSE")
                && rule.contains("[Open thread](thread:<ws>/<uuid>)"),
            "rule must exempt markdown link targets so spawned-thread links survive:\n{rule}"
        );
    }

    /// Regression guard for the 2026-08-06 incident. The rule has to carry two
    /// things, and the second is the one that was actually missing on the day:
    /// the prohibition, and what to do instead. An agent that only knows it may
    /// not work around a tool still has to be told that reporting the block IS
    /// the correct turn, because the failure was silence plus a workaround, not
    /// a misunderstanding of the tool's scope.
    #[test]
    fn no_impersonation_rule_bans_the_workaround_and_names_the_honest_move() {
        let rule = NO_IMPERSONATION_RULE;
        for needle in [
            "NEVER ACT AS THE USER, AND NEVER ROUTE AROUND A TOOL",
            "curl",
            "as though the user typed it",
        ] {
            assert!(
                rule.contains(needle),
                "rule must ban the workaround (`{needle}`):\n{rule}"
            );
        }
        assert!(
            rule.contains("WHEN A TOOL REFUSES YOU, SAY SO"),
            "rule must name the honest move, not just the ban:\n{rule}"
        );
        assert!(
            rule.contains("not possible") && rule.contains("offer what IS"),
            "rule must tell the agent to report the block AND offer an alternative, \
             or a refusal just becomes a dead end:\n{rule}"
        );
        // The engine refuses these requests too, but a rule that leans on the
        // block invites hunting for the gap. Say the block exists and then say
        // not to probe it.
        assert!(
            rule.contains("do not go looking for the edge it misses"),
            "rule must not read as an invitation to find the engine's gap:\n{rule}"
        );
    }

    /// Regression guard for the reported card: a timezone question whose
    /// fourth option was "Other, I'll type it". Lucidos has no text-entry
    /// option, so tapping it returns that label as the user's answer. The rule
    /// must ban the option, explain WHY (the label comes back as the answer),
    /// and name both real escapes, or the agent keeps inventing the button to
    /// fill a gap that does not exist.
    #[test]
    fn ask_user_question_rule_bans_a_text_entry_escape_option() {
        let rule = ASK_USER_QUESTION_RULE;
        for needle in [
            "NEVER OFFER AN \"OTHER\" OPTION",
            "Let me type it",
            "no text-entry option",
            "prompt textarea",
            "Cancel dismisses the question",
        ] {
            assert!(
                rule.contains(needle),
                "rule must ban the escape-hatch option and name the real escapes (`{needle}`):\n{rule}"
            );
        }
        // The sentence that produced the button lived in the `ask_user_question`
        // tool description (retired there, pinned by its own test). Guard
        // against it migrating here, where it would contradict the ban above.
        assert!(
            !rule.contains("Always include an option that lets the user opt out"),
            "the retired opt-out instruction must not reappear in the chat rule:\n{rule}"
        );
        // An option carrying a real decision is NOT what this bans. Without the
        // carve-out the agent reads it as "never offer an out" and drops
        // legitimate Cancel-style choices.
        assert!(
            rule.contains("None of these") && rule.contains("still welcome"),
            "rule must keep a meaningful opt-out option legal:\n{rule}"
        );
    }

    /// Abbreviation is a `$HOME`-prefix collapse, not a blanket redaction — a
    /// workspace mounted outside the home dir keeps its real path.
    #[test]
    fn workspace_outside_home_keeps_its_absolute_path() {
        let outside = Path::new("/srv/lucidos/ws");
        // Guard the (absurd but cheap to rule out) case of $HOME being /srv.
        let home = std::env::var("HOME").expect("HOME must be set to run this test");
        assert!(!outside.starts_with(&home), "test fixture overlaps $HOME");

        let section = workspace_identity_section("shared", outside, "", "");
        assert!(section.contains("WORKSPACE: shared (/srv/lucidos/ws)"));
    }

    /// The 2026-08-06 miss, pinned. Asked to be told "here" when a coding agent
    /// edits code, the agent went looking for a trigger, wrote "a trigger can't
    /// write into this chat thread" as a caveat, and asked which granularity of
    /// trigger to build. Every surface framed `await_event` as an anti-polling
    /// remedy, and the only stated test (one-shot vs standing) picks a trigger
    /// for that sentence.
    ///
    /// Three things have to hold together. The destination question must be
    /// present, the duration question must SURVIVE beside it (a rule that only
    /// says "in this thread means await_event" pushes a genuine standing rule
    /// into a one-shot wait), and neither may overclaim: a trigger reaches the
    /// user perfectly well, just not inside the conversation they typed into.
    #[test]
    fn trigger_vs_event_wait_rule_routes_on_destination_without_overclaiming() {
        let rule = TRIGGER_VS_EVENT_WAIT_RULE;

        // Destination: the axis that was missing everywhere.
        assert!(
            rule.contains("WHERE"),
            "the destination question has to be asked explicitly:\n{rule}"
        );
        assert!(
            rule.contains("cannot continue the conversation you are in"),
            "the fact the agent derived and then ignored must be stated as the \
             disqualifier, not left to be re-derived:\n{rule}"
        );
        assert!(
            rule.contains("even when the phrasing sounds like a standing rule"),
            "the miss was phrasing that reads as standing, so the rule has to \
             pre-empt exactly that reading:\n{rule}"
        );

        // Duration: the axis that already existed, which must not be lost.
        assert!(
            rule.contains("one-shot") && rule.contains("outlive this conversation"),
            "dropping the duration half routes standing rules into a wait that \
             stops after one match:\n{rule}"
        );

        // Both mechanisms named, so neither half is a dead end.
        assert!(
            rule.contains("await_event") && rule.contains("trigger"),
            "a fork has to name both branches:\n{rule}"
        );

        // No overclaim. A trigger notifying the user is its normal, correct
        // behaviour; only in-conversation delivery is out of reach.
        assert!(
            rule.contains("reaches the user as a notification"),
            "must say what a trigger DOES do, or it reads as 'triggers cannot \
             tell me anything':\n{rule}"
        );

        // The heading is a cross-reference anchor: the "WAITING FOR A STATE
        // CHANGE IN LUCIDOS" section points the model back here by name, and
        // that pointer is the only route from the duration half to this one.
        assert!(
            rule.starts_with("\"TELL ME WHEN X HAPPENS\""),
            "the state-change section names this block by its opening heading; \
             renaming it here silently dangles that pointer:\n{rule}"
        );
    }
}
