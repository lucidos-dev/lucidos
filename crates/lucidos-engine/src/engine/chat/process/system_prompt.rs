//! Chat-agent system-prompt assembly. Builds the full per-turn system prompt
//! (identity, timezone, the large static base prompt, apps/intents/knowhow
//! listings, images section, trigger framing) plus the mandatory-setup
//! preference scan.

use crate::core::PreferenceStore;
use crate::engine::LucidosEngine;
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
/// The engine refuses a no-`folder` spawn here, but the model must be TOLD as
/// well as blocked. Otherwise it narrates the capability first and discovers
/// the refusal after the user has already been misled.
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

/// Pick the coding-surface prompt section for this install. The single
/// divergence point between the two system-prompt variants.
///
/// The caller supplies [`crate::paths::has_lucidos_source`], the same signal
/// behind the `/health` `packaged` flag. The compose picker and the chat agent
/// therefore cannot disagree about whether Lucidos source exists.
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
/// choice-shaped question instead of writing the options as plaintext markdown.
///
/// The "NEVER OFFER AN \"OTHER\" OPTION" paragraph is load-bearing, not
/// stylistic. Lucidos has no text-entry option kind, and
/// `agent_question::answer_kind_to_hook_value` resolves a `Selected` answer to
/// the option's LABEL. So an "Other, I'll type it" button hands that literal
/// phrase back as the user's decision.
///
/// The WAKE QUESTION paragraph is scoped to "no Lucidos event to subscribe to".
/// Unscoped it contradicts the long-running-work section, which routes a
/// background task to `await_event` on `BackgroundBashCompleted`.
///
/// Mirrored for coding agents by `agent_session::prompts::{ASK_USER_QUESTION_RULE,
/// CODEX_ASK_USER_QUESTION_RULE}` and by the `ask_user_question` tool
/// description in `llm::tools::misc`. Change them together.
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
/// identifier. No Lucidos surface is labelled with a change id or a sha, so a
/// question that quotes one is unanswerable.
///
/// Two carve-outs keep the rule presentation-only, and both are load-bearing.
/// Ids must stay legal in tool arguments, or the model stops passing
/// `change_id` to `changes(action='apply')`. And a **markdown link target** is
/// not prose. `run_thread` and `run_coding_agent` results tell the agent to
/// reply with `[Open thread](thread:<ws>/<uuid>)`, which is the user's only
/// affordance for opening the thread that was just spawned.
///
/// The trigger form is named there for a reason beyond completeness. Told to
/// link a trigger, the agent reads the ban as covering the target's id, and
/// reaches for the panel link instead. That is the reported bug
/// (docs/plans/2026-08-24-a-trigger-is-a-link.md), so the carve-out says which
/// half of a link the name goes in.
///
/// Mirrored for coding agents by `agent_session::prompts::NAMES_NOT_IDS_RULE`,
/// tuned there to commits and worktrees. Change both together.
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
     result gives it to you, `[Habit Tracker](app:habit-tracker)` for an app, \
     `[Nightly digest](trigger:<id>)` for a trigger. The NAME goes in the label, the id in the \
     target. Dropping one because it holds a uuid leaves the user no way to open it.\n\
     ONE EXCEPTION: the user asked for the raw value, or needs it to paste somewhere. Then give \
     it, wrapped in <copy>…</copy> tags.";

/// Stops the chat agent from CLAIMING it performed an action it never actually
/// invoked the tool for.
///
/// The CRITICAL RULES block carries the same claim scoped to `write_file` and
/// `edit_file`. This rule generalizes it to every state-changing tool and names
/// the repeat-request trap directly. Logged as a model-tolerance measure in
/// `docs/temporary-measures.md`.
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
/// The engine refuses all three moves (`api::chat::human_mode_is_attributed`,
/// `api::chat::thread_target_is_addressable`, `api::target_workspace`). The
/// model must still be TOLD, for the same reason [`NO_LUCIDOS_SOURCE_SECTION`]
/// exists. The load-bearing half is not the prohibition but the instruction
/// about what to do instead: report the block.
///
/// Deliberately short. It points at the tools that do work rather than
/// restating their contracts, which `system-knowhow/lucidos-cli.md` owns.
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
/// The destination question is stated here, alongside the duration question
/// rather than replacing it, because it is resident in every chat turn. Every
/// other surface frames `await_event` as an anti-polling remedy. The one
/// discriminator they all state, one-shot versus standing, picks a trigger for
/// a request phrased as a standing rule. The knowhow carries the same fork,
/// but knowhow has to be RETRIEVED, which is often too late.
///
/// Deliberately does not claim a trigger cannot reach the user at all: it can,
/// as a notification from its own thread. What it cannot do is continue the
/// conversation the user typed into. Pinned by
/// `trigger_vs_event_wait_rule_routes_on_destination_without_overclaiming`.
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
/// one. Narrowing the trigger row back to create/update leaves "run my digest
/// trigger now" with no route. The agent then guesses and reaches for resume,
/// which restores the schedule and runs nothing. Pinned by
/// `workspace_assets_rule_routes_existing_trigger_operations`.
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

/// Routes a plugin *setup thread* to the `plugin-setup` knowhow.
///
/// The engine seeds that thread itself, so the opening request has a KNOWN
/// shape: `build_setup_thread_request` in `engine::tools::plugins` writes one
/// line for a fresh install and another for an update. Both keep the verb "set
/// up", and this route has to recognise both. Reword either side alone and the
/// thread still starts, still looks right, and quietly gets a generic answer
/// instead of the recipe. Pinned by
/// `both_setup_seeds_route_to_the_plugin_setup_knowhow`.
///
/// Named rather than inline so that test can read it. The re-run clause reads
/// as filler and is not: it is what stops an update re-asking everything the
/// last run already settled.
///
/// It is also SHORTER than the install-only line it replaces, because this text
/// is billed on every turn of every thread. Naming a second occasion bought the
/// room by dropping the summary of steps the knowhow itself owns.
pub(crate) const PLUGIN_SETUP_RULE: &str =
    "SETTING UP A NEW OR UPDATED PLUGIN, LOAD KNOWHOW FIRST:\n\
     When asked to set up a newly installed or updated plugin, FIRST \
     load_knowhow('system-knowhow/plugin-setup') and follow it: it owns reusing \
     an earlier run, the author's instructions, and doing the steps with the \
     user. Skip the load if you already loaded it in this thread.";

/// Routes "set Lucidos up around my life" to the *setup interview* knowhow.
///
/// The first-run entry point sends a fixed sentence as an ordinary user
/// message, so the opening request has a KNOWN shape. That earns a
/// deterministic route rather than a bet on frontmatter `description` winning
/// retrieval. The description covers the re-run phrasings a returning user
/// types.
///
/// The ACTION FIRST carve-out is load-bearing, not boilerplate. That rule says
/// "Don't ask clarifying questions" and excepts only workspace assets, while an
/// interview is a clarification loop by construction. Without the carve-out the
/// agent skips the questions and builds something generic on turn one. Pinned
/// by `setup_interview_rule_carves_itself_out_of_action_first`.
///
/// The "not only about work" sentence is here rather than in the knowhow. This
/// block is what the model reads BEFORE deciding to load anything.
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

/// The todo list where the `todo_write` tool exists, which is everywhere the
/// *self-curated context mode* is off.
const TODO_LIST_RULE: &str = "TODO LIST (todo_write, live progress in the prompt bar):\n\
     - Use it for a multi-step request (3+ steps) so the user watches the plan \
     tick along; skip it for trivial single-step work.\n\
     - Call once at the start with every item `pending`, then re-call flipping \
     one to `in_progress` before you start it and to `completed` when it's done.";

/// The same list under the mode, which withdraws `todo_write` from the schema.
///
/// One list, one write surface. Ordering a call the array does not offer is
/// what this replaces: the model met the instruction on every mode-on turn and
/// had nothing to call.
const TODO_LIST_RULE_CONTEXT_MODE: &str =
    "TODO LIST (a [TODO] heading in your working understanding, live progress \
     in the prompt bar):\n\
     - Use it for a multi-step request (3+ steps) so the user watches the plan \
     tick along; skip it for trivial single-step work.\n\
     - Write the whole list under the heading, and rewrite it as items move. \
     The mode's own section below owns the marks. There is no todo tool here.";

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
Change an intent only when the user's goal changes, and never put technical detail in one. Write the knowhow file BEFORE the intent that needs it, so the procedure has somewhere to live.

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

__PLUGIN_SETUP_RULE__

__SETUP_INTERVIEW_RULE__

ACTION FIRST, NO CLARIFICATION LOOPS:
- If the user asks for something, DO IT. Resolve ordinary vagueness yourself ("this week" is since Monday), and act on a request that is 80% clear. If you're wrong the user will say so, which is faster than a clarification loop.
- NEVER ask "do you mean X?" or "just to clarify". "Show me this week's tasks" is answered by showing them, not by asking whether that means since Monday or the last 7 days.
- Only ask when you genuinely cannot proceed. Two rules above override this one: WORKING WITH WORKSPACE ASSETS, and the setup interview.

__ASK_USER_QUESTION_RULE__

__TODO_LIST_RULE__

TOOLS: Use efficiently, don't loop. One call per file, and don't re-read a file you just wrote. Prefer edit_file over write_file for an existing file, and its json_path + new_value mode for a .json or .slides file. edit_file answers "N of M occurrences replaced": N < M means the rest are still there.

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

SEARCHING FILES, AND WORKING ON CODE:
- glob_files finds files by path pattern and grep_files regex-searches contents, both relative to data/. Prefer them over run_bash with find, rg or grep: structured, faster, and they respect workspace boundaries.
- A REGISTERED REPOSITORY IS REACHABLE THE SAME WAY: pass `repo` to read_file, glob_files, grep_files or edit_file. NEVER `cd` into a checkout to cat, sed, grep or awk it, which returns raw untruncated bytes and walks target/.
- A repo edit needs `commit: false` and commits NOTHING: say so, and point at git diff. run_coding_agent instead for anything adding or deleting files, or that should land as a reviewable change.

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
- Whether the watch is one-shot is the duration half of the choice; the destination half is TELL ME WHEN X HAPPENS above, which decides "let me know HERE".
- IT WATCHES THE WHOLE WORKSPACE, not just this thread, so ANY thread's completion is a first-class wait, someone else's coding-agent session included: `CodingAgentIdled` with a `condition` of `{"thread_id": "<uuid>"}`, which scopes ANY thread event to one thread. NOT YOUR OWN CHILD: it already re-opens this thread with its status, summary and pending change ids, so a wait on it is a second wake. Say you'll report back, and end the turn.
- "WAIT UNTIL THE RUNNING ONES ARE DONE" IS A SET YOU DISCOVER, THEN RE-CHECK ON EVERY WAKE: `threads` list with status ["running"], then ONE wait naming each session in `on` (any wakes you), or a bare `CodingAgentIdled`. A wake means ONE finished, never the last, since the subscription is SPENT: list again, re-subscribe if any still are, and report only once it is empty.
- "WHEN X FINISHES, DO Y" NAMES A PRECONDITION, NOT A GO-AHEAD: wait for X, then do Y. "Auto approved" pre-approves Y for when X has happened; it never replaces X.
- IT RETURNS IMMEDIATELY AND BLOCKS NOTHING, so say what you're waiting for and END YOUR TURN. The tool's own schema carries the rest.
- NEVER ANSWER "AM I STILL WATCHING FOR THAT?" FROM MEMORY: call `list_event_waits`, and `cancel_event_wait` to STOP.

REFRESHING OPEN WINDOWS:
- A file the user has open refreshes itself the moment you write it, and app UIs refresh when their files change. There is no file-refresh tool, so don't spend a step looking for one and don't tell the user to refresh. (refresh_app is a different thing and still exists.) If the file isn't open, just mention the path; it renders as a clickable link.

FILE REFERENCES:
- Always use the full path ("artifacts/notes.md", not "notes.md"): a full path becomes a clickable link, a bare filename does not.
- LINK EVERY APP YOU NAME, since a bare app name does not auto-link: `[Habit Tracker](app:habit-tracker)`. Not linking should be a rare exception.
- LINK EVERY TRIGGER YOU NAME, at the trigger and not the panel: `[Nightly digest](trigger:<id>)`, id from `triggers` action 'list'. It lands on the row, where Run once is. `[Triggers](triggers)` is the LIST.
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

/// Resolve every placeholder token in [`SYSTEM_PROMPT_BASE`] and append the
/// coding-surface section, yielding the workspace-independent body of the chat
/// system prompt.
///
/// `max_tool_calls` is this turn's resolved cap, passed in rather than read
/// here so it is the SAME number `run_agentic_loop` enforces. A prompt naming a
/// number the loop does not use is exactly the fabricated engine internal the
/// ENGINE INTERNALS section warns against.
///
/// `mode` is here for the same reason. The mode withdraws `todo_write`, so a
/// body that cannot vary orders a call the tools array does not offer.
fn static_prompt_body(
    has_lucidos_source: bool,
    max_tool_calls: usize,
    mode: super::context_mode::ContextMode,
) -> String {
    let apply_verify_rule = if has_lucidos_source {
        format!("{}{}", APPLY_VERIFY_RULE, APPLY_VERIFY_DEV_ADDENDUM)
    } else {
        APPLY_VERIFY_RULE.to_string()
    };
    let todo_list_rule = if mode.is_on() {
        TODO_LIST_RULE_CONTEXT_MODE
    } else {
        TODO_LIST_RULE
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
        .replace("__PLUGIN_SETUP_RULE__", PLUGIN_SETUP_RULE)
        .replace("__SETUP_INTERVIEW_RULE__", SETUP_INTERVIEW_RULE)
        .replace("__TODO_LIST_RULE__", todo_list_rule)
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
    /// Build the full chat system prompt for this turn, plus the mandatory
    /// missing-preference keys.
    ///
    /// Reads no clock, no build state and no request origin, deliberately: the
    /// result is a cached prefix tier, so anything volatile in it costs a full
    /// rewrite per turn. `super::turn_clock` and `super::turn_tail` own that
    /// rule and the tail blocks the readings moved to.
    pub(super) async fn build_chat_system_prompt(
        &self,
        user_timezone: &str,
        user_language: &str,
        device_id: Option<&str>,
        // `Some` on a trigger fire, which is the whole of what "is this a
        // trigger" means to the caller too (`trigger.is_some()`).
        trigger: &Option<TriggerContext>,
        // This turn's resolved tool-call cap. Passed in rather than read here
        // so it is the SAME number `run_agentic_loop` enforces.
        max_tool_calls: usize,
        // This workspace's tool gates and the intent snapshot behind one of
        // them, read once per turn by the caller. Shared so the prompt and the
        // tools array cannot disagree about what this workspace can do.
        capabilities: &crate::engine::tools::TurnCapabilities,
    ) -> (String, Vec<&'static str>) {
        // Timezone RULES only. The reading itself rides at the tail of the user
        // message, so this whole block stays byte-identical turn to turn and
        // thread to thread. See `super::turn_clock`.
        let timezone_section = super::turn_clock::timezone_section(user_timezone);

        // (key, instruction, per_device). Any missing key flips this turn into
        // setup mode.
        let mandatory_prefs: &[(&str, &str, bool)] = &[
            ("timezone", "- TIMEZONE: Ask what timezone they are in and call preferences(action=\"set\", key=\"timezone\", value=\"…\") with an IANA name (e.g., \"America/New_York\", \"Europe/London\", \"Asia/Tokyo\").", false),
            ("language", "- LANGUAGE: Ask what language they prefer, and mention that English is recommended for best results (the models are strongest in English) — but they can still write in any language; replies come back in whichever language they set here. Then call preferences(action=\"set\", key=\"language\", value=\"…\") to save it.", false),
            ("push_notifications", "- PUSH NOTIFICATIONS: Ask if they want to enable push notifications for scheduled task alerts (do NOT call them \"browser\" notifications — Lucidos runs as a native desktop app too, where these are native OS alerts). When you describe how they arrive, key off the current request device in [USER DEVICE & PREFERENCES]: if its details say \"Lucidos desktop app\" they are in the native desktop app (native macOS notifications, no browser or site permission); otherwise they are in a browser/PWA (the browser will prompt for permission). If yes, call preferences(action=\"set\", key=\"push_notifications\", value=\"enabled\"). If no, call preferences(action=\"set\", key=\"push_notifications\", value=\"declined\") so you don't ask again.", true),
        ];

        let mut missing_instructions = Vec::new();
        let mut missing_pref_keys = Vec::new();
        for (key, instruction, per_device) in mandatory_prefs {
            // A read that FAILED is not a preference that is unset. Collapsing
            // both into `None` flips the whole turn into "SETUP REQUIRED, DO
            // NOT PROCEED" below, so one transient DB error refuses the user's
            // actual request. Treat an unreadable key as configured: a missed
            // setup nag costs a prompt, a false refusal costs the turn.
            let read = if *per_device {
                if let Some(did) = device_id {
                    PreferenceStore::get_for_device(&self.pool, key, did).await
                } else {
                    // No device context (child thread, scheduled task), so a
                    // per-device preference is irrelevant here.
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

        let has_lucidos_source = crate::paths::has_lucidos_source();

        // Read once and used twice, by the body and by the mode's own section
        // below. Two reads of the same snapshot could not disagree, but one
        // name says plainly that the two are the same mode.
        let context_mode = super::context_mode::ContextMode::from_capabilities(&capabilities.gates);

        let system_prompt = format!(
            "{}{}",
            system_prompt,
            static_prompt_body(has_lucidos_source, max_tool_calls, context_mode)
        );

        // Whether the user has restarted onto the newest build is a FACT the
        // engine holds. It is stated rather than left to be inferred from
        // having applied a `requires_restart` change earlier in the thread. The
        // prescribed probe (fetch a served asset) cannot answer for a
        // backend-only change, which alters no served asset. Dev-only, on the
        // same gate as the apply-verify addendum that reads it.
        //
        // Only the POINTER is here. The state itself is a per-turn value, so it
        // rides in the message tail (`super::turn_tail`).
        let system_prompt = if has_lucidos_source {
            format!(
                "{}{}",
                system_prompt,
                super::turn_tail::ENGINE_BUILD_POINTER
            )
        } else {
            system_prompt
        };

        // Same split, same reason: the URL comes from the last observed request
        // origin, so the cached tier gets the instruction and the tail gets the
        // reading.
        let system_prompt = format!("{}{}", system_prompt, super::turn_tail::CLIENT_URL_POINTER);

        // No auto-detected browser-login domain list here, deliberately:
        // `docs/adr/0067-browser-login-domains-never-reach-the-prompt.md`.

        // A read that FAILED is not a workspace with no apps, and the prompt
        // renders the two identically. So the agent tells the user they have
        // none, and nothing anywhere says the scan broke. Same treatment as the
        // preference read above: degrade the turn, but name what happened.
        let apps_section = match self.app_manager.list_apps() {
            Ok(apps) => super::workspace_payload::build_apps_section(&apps),
            Err(e) => {
                log!(
                    "[Chat] app list read failed ({}); this turn's prompt names no apps",
                    e
                );
                String::new()
            }
        };

        // The SAME snapshot the `execute_intent` gate was derived from, not a
        // second scan of `data/`. Two scans can disagree inside one turn, and
        // one direction is harmful: an intent created between them lists here
        // while the tool to run it was never offered.
        let intents_section = if capabilities.intents.is_empty() {
            String::new()
        } else {
            let mut section = String::from("\n\n## Available Intents\n\n");
            section.push_str("Stored descriptions of what the user wants. Use execute_intent(intent_id) to fulfill one. Each intent is paired with knowhow that tells you how to achieve it.\n\n");
            for p in &capabilities.intents {
                section.push_str(&format!("- **{}** (id: `{}`)\n", p.name, p.id));
            }
            section
        };

        let kh_dirs = self.knowhow_dirs();
        let knowhow_summaries = crate::core::KnowhowStore::load_merged_summaries(&kh_dirs);
        let apps_dir = self.workspace_path.join(crate::core::APPS_DIR);
        let app_knowhow_summaries = crate::core::KnowhowStore::load_app_summaries(&apps_dir);
        let knowhow_section = super::workspace_payload::build_knowhow_section(
            &knowhow_summaries,
            &app_knowhow_summaries,
        );

        // Engine-shipped reference, never overrideable by a workspace file.
        let system_knowhow_summaries = self
            .system_knowhow_dir()
            .map(crate::core::SystemKnowhowStore::load_summaries)
            .unwrap_or_default();
        let system_knowhow_section = build_system_knowhow_section(&system_knowhow_summaries);

        let system_prompt = format!(
            "{}{}{}{}{}",
            system_prompt, apps_section, intents_section, knowhow_section, system_knowhow_section
        );

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
            if capabilities.gates.image_provider {
                section.push_str(" You can also generate or edit images with the generate_image tool. \
                    To edit an existing image, reference it as 'thread:N' where N is its position in the thread, \
                    or use an artifact path like 'artifacts/photo.png'. \
                    When the user says \"edit the second image\", use input_images: [\"thread:2\"].");
            }
            section
        };

        // Appended last so the unconditional sections above stay byte-identical
        // between trigger fires and regular chats. That shared prefix is what
        // the provider's prompt cache keys on. The per-trigger knowhow listing
        // is trigger-only for the same reason, so it lives here rather than in
        // the unconditional knowhow section above.
        let system_prompt = match trigger {
            Some(t) => {
                let triggers_dir = self.workspace_path.join(crate::core::TRIGGERS_DIR);
                format!(
                    "{}{}{}",
                    system_prompt,
                    build_trigger_knowhow_section(&triggers_dir, &t.slug),
                    crate::scheduler::user_tasks::TRIGGER_SYSTEM_ADDENDUM
                )
            }
            None => system_prompt,
        };

        // ADR 0085 decision 8: the standing re-read rule belongs in the cached
        // prefix, because it never changes. Empty when the mode is off, so the
        // control arm's prompt is byte-identical to a build without the mode.
        let system_prompt = format!(
            "{}{}",
            system_prompt,
            super::context_mode::system_prompt_section(context_mode, capabilities.schedule)
        );

        (system_prompt, missing_pref_keys)
    }
}

/// Build the prompt's opening identity block: who the agent is, which
/// workspace it runs in, the timezone and language sections, and the
/// personal-data-access framing.
///
/// The workspace path is rendered through [`crate::core::home_path::abbreviate`].
/// On an MDM-managed fleet the home dir is named `<username>@<employer-domain>`.
/// A raw absolute path there ships the user's employer to the model provider on
/// every turn. Display text only: the engine's own path handling is unaffected.
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
    use super::super::context_mode::ContextMode;
    use super::super::turn_tail::{
        client_url_block, engine_build_block, version_status, BUILD_STATES, CLIENT_URL_POINTER,
        ENGINE_BUILD_POINTER,
    };
    use super::{
        coding_surface_section, static_prompt_body, workspace_identity_section,
        ASK_USER_QUESTION_RULE, NAMES_NOT_IDS_RULE, NO_IMPERSONATION_RULE, SETUP_INTERVIEW_RULE,
        TRIGGER_VS_EVENT_WAIT_RULE, WORKSPACE_ASSETS_KNOWHOW_RULE,
    };
    use crate::llm::ToolCapabilities;
    use std::path::{Path, PathBuf};

    /// Ceiling on the engine-authored text every chat turn pays for before the
    /// user has typed a word. [`always_loaded_areas`] lists what is billed.
    ///
    /// A RATCHET, not a target. Every character here is billed on every request
    /// of every thread in every workspace, so growth is never free. A new rule,
    /// tool or knowhow file pays for itself by trimming elsewhere. Otherwise
    /// this number moves in a change that says why it is worth it.
    ///
    /// The ratchet is deliberately tight. A pass that reclaims space lowers
    /// this line in the same change. Otherwise the reclaimed space is silently
    /// spent by the next thing that grows.
    ///
    /// Paired with `no_single_tool_schema_dominates_the_always_loaded_budget`,
    /// because this number alone lets one runaway schema hide behind twenty
    /// lean ones.
    ///
    /// Raised by 339 for ADR 0088's second half, to a measured 108,589. Both
    /// halves buy a stable cached tier, and the gates in the same change move
    /// NOTHING here: this meter bills the engine-authored surface, so
    /// [`flat_chat_tools`] resolves every gate open.
    ///
    /// - **135** for the `ENGINE BUILD` split. One 374-character section
    ///   became a 307-character pointer plus a 202-character tail block: the
    ///   pointer says where the reading is, the block says what it reads. In
    ///   return the four states stop rewriting the whole 21,668-token system
    ///   tier when the build flips mid-window.
    /// - **204** for the client-URL sentence, which was never in this meter at
    ///   all. It is always-loaded text and now says so.
    ///
    /// Raised by 1,893 for ADR 0093, to a measured 110,522. It buys the `repo`
    /// argument on four file tools and `edit_file`'s `commit` (1,362), plus the
    /// prompt lines routing code work to them (531). This is the rare addition
    /// that PAYS for itself inside the same window. No file tool could reach a
    /// code repo at all, so the agent read one through `run_bash` with `sed`
    /// and `grep -rn`: 115 bash calls in one measured nightly thread, against 4
    /// `grep_files`. Each of those spends far more context in untruncated
    /// output than the schemas cost per turn. Trims in the same change gave
    /// 627 back.
    ///
    /// Raised by 2,432 for ADR 0085's context mode, to a measured 112,954:
    /// 2,053 for its re-read rule and note guidance, 379 for `todo_write`'s
    /// `notes` property. The largest raise here, and it buys an EXPERIMENT.
    /// None of it reaches a workspace with the mode off, which today is every
    /// one. It comes out again when the eval reports
    /// (`docs/temporary-measures.md`). The meter bills it anyway, by the rule
    /// that opens every tool gate in [`flat_chat_tools`].
    ///
    /// Raised by 1,319 more when the model took over that curation, to a
    /// measured 114,273. Four parts, in
    /// `docs/plans/2026-08-20-model-curated-context-mode.md`:
    ///
    /// - **136** for the body region, which has to say where the bodies sit.
    /// - **223** for a recovery table that went from two rows to five.
    /// - **909** for version 2 of the note guidance, which now states the
    ///   release half of the loop as well as the note-writing half.
    /// - **51** for `dismiss_from_context`'s widened schema.
    ///
    /// Only the last reaches a workspace with the mode off, a tool schema not
    /// being gated on the flag. The rest goes when the experiment does.
    ///
    /// Raised by 539 when the mode inverted its default, then by 471 for the
    /// prose that inversion needs, to a measured 115,283
    /// (`docs/plans/2026-08-21-persist-on-demand-context-mode.md`):
    ///
    /// - **539** for `keep_in_context`, the verb that states the model still
    ///   wants a body.
    /// - **471** for version 3 of the note guidance and the mode's rule block,
    ///   which now state a deadline rather than an option.
    ///
    /// None of it reaches a workspace with the mode off. The keep verb is
    /// shaped OUT of a mode-off array, so unlike its opposite it is not even
    /// offered there. The meter bills both anyway, by the rule that opens
    /// every gate in [`flat_chat_tools`].
    ///
    /// Raised by 239 when `run_coding_agent`'s `model` gained a JSON-schema
    /// enum, to a measured 115,522. The union of both backends' model ids is
    /// 317 chars of frozen shape. No description could state it instead, and
    /// it replaced examples the description no longer has to carry.
    ///
    /// Raised by 94 for the one sentence ordering the knowhow write before the
    /// intent, to a measured 115,616. The block already said never to put
    /// technical detail in an intent. A model holding procedure with nowhere
    /// to put it inlined it anyway. Both eval arms did that on T05, having
    /// read the longer rule first. A prohibition needs the step that makes
    /// obeying it possible, and that step is worthless after the tool call.
    ///
    /// Raised by 187 for `run_bash`'s never-detach clause, to a measured
    /// 115,803. `&` binds looser than `&&`, so a backgrounded chain holds the
    /// call's pipes. The tool used to report a timeout for a launch that had
    /// already succeeded (ADR 0100). The fix reports the survivor, but the
    /// agent still has to not write it. `running-python.md` carries the full
    /// anti-pattern, and a model reaching for `run_bash` never loads that.
    ///
    /// Raised by 6 for the seventh provider name in `manage_models`, to a
    /// measured 115,809. `opencode-free` is one value in a closed enum the
    /// tool validates against, and a name the agent must be able to send.
    /// Adding a provider without it would make the enum lie.
    ///
    /// Raised by 635 for ADR 0109, to a measured 116,444. The mode's fixed
    /// rent went UP, and the plan expected it to fall, so the split is worth
    /// stating:
    ///
    /// - **+575** in tool schemas. `scratchpad` arrived and
    ///   `dismiss_from_context` left, and the note verb is the larger schema.
    /// - **+60** in the mode's rule block and note guidance, which now state a
    ///   one-round deadline and three answers instead of a release rule.
    ///
    /// What fell is not on this meter. The context ledger and the body region
    /// rode in the user message of every lean request, sized with the bodies
    /// they described. All of that is gone. This
    /// meter bills the fixed half only, so it shows the new tool and not the
    /// variable block that paid for it.
    ///
    /// Earlier raises are in `git blame`, each with its reason in its own
    /// commit message. The three most recent: **252** for the trigger link
    /// form, 6 for `opencode-free`, and 187 for `run_bash`'s never-detach
    /// clause.
    ///
    /// The 252 buys a link form that did not exist. A trigger was the one
    /// first-class thing the agent names with no way to link it, so it linked
    /// the Triggers panel instead (ADR 0112). Two places pay: the FILE
    /// REFERENCES bullet carrying the form, and one clause in
    /// [`NAMES_NOT_IDS_RULE`] exempting the id in its target. Both are resident
    /// because the choice is made in the reply itself, where no knowhow load
    /// can reach.
    ///
    /// Raised by 149 for ADR 0113, to a measured 116,845. A persisted system
    /// event is now awaitable, so `await_event` states which families the gate
    /// admits and which frames it refuses. The knowhow cannot carry it: the
    /// name is chosen in the call, and a refused one spends a turn.
    ///
    /// **LOWERED to a measured 115,112** by the self-curated context mode.
    ///
    /// The meter bills the mode OFF, which is the larger array: the mode adds
    /// no tool and withdraws `todo_write`. That is the same worst-case rule
    /// this meter always applied, reaching the other arm. See
    /// [`flat_chat_tools`].
    ///
    /// Raised by 164 to a measured 115,276, for the `events` tool's fourth
    /// action, `event_types`. A subscription now refuses a name the engine
    /// never emits, and that refusal has to send the caller somewhere. This is
    /// the only surface answering which names exist here.
    ///
    /// Raised by 63 to a measured 115,339, for one sentence of the mode's rule
    /// block that was false. It told the model a finished turn's tool calls are
    /// never sent back. The resume builder rebuilds the last few of them, with
    /// their addresses, on every follow-up turn. So the model was told to price
    /// a round it was not being charged for, and told nothing about the one it
    /// was. The true sentence runs 63 characters longer.
    ///
    /// Raised by 230 to a measured 115,569 by ADR 0119. A condition key became
    /// a *field path* and gained `$nin`, `$regex` and `$or`. The vocabulary is
    /// resident because it is chosen while writing the call: a condition naming
    /// an operator that does not exist is refused, and one naming a field the
    /// old way silently matches nothing.
    /// Raised by 10 to a measured 115,579 for `navigate_ui`'s `release-notices`
    /// settings view. *Release notices* moved out of What's New onto their own
    /// System tab. The agent has to be able to send a user to the tab that owes
    /// them work. Every other System subpanel but Communication Surfaces is
    /// already reachable that way.
    const ALWAYS_LOADED_BUDGET_CHARS: usize = 115_579;

    /// The hand-written flat tool schemas the chat agent is offered.
    ///
    /// Mirrors the tool splice in `chat::process::run`, minus the two parts
    /// that are not engine-authored text: tools discovered from running MCP
    /// servers, and `generate_image` (present only when an image provider is
    /// configured). The grouped manifest tools are billed separately because
    /// they have a different owner, `crate::capability_manifest`.
    ///
    /// **Every capability gate is resolved OPEN here except the image one**,
    /// which is the same set this measured before ADR 0088 gated anything.
    /// The meter is the engine-authored surface, not one workspace's array, so
    /// a gate closing somewhere reclaims nothing this ratchet may spend.
    ///
    /// The context mode is the exception, and it is that same rule applied.
    /// The mode CLOSES a family rather than opening one: it takes `todo_write`
    /// away. Billing it on would reclaim a schema almost every workspace pays
    /// for, so the meter bills the mode OFF, which is the larger array.
    fn flat_chat_tools() -> Vec<crate::llm::provider::ToolDefinition> {
        let billed = ToolCapabilities {
            email_account: true,
            intent: true,
            image_provider: false,
            context_mode: false,
        };
        let mut flat = crate::llm::tools::get_default_tools(&billed);
        flat.push(crate::llm::tools::get_notification_tool());
        flat.extend(crate::llm::tools::chat_tail_tools(&billed));
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
        // Neither source variant is uniformly larger, so measure both and bill
        // the worse case.
        //
        // The mode is billed OFF, for the reason `flat_chat_tools` bills it
        // off: that is the configuration almost every workspace runs. Its
        // todo rule is 31 chars longer, against a `todo_write` schema of
        // nearly a thousand that a mode-on workspace does not pay at all.
        let body = std::cmp::max(
            static_prompt_body(true, 500, ContextMode::Off)
                .chars()
                .count(),
            static_prompt_body(false, 500, ContextMode::Off)
                .chars()
                .count(),
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

        // Both halves of each ADR 0084 split, because both are always-loaded:
        // the pointer in the cached system block and the reading in the
        // message tail. Built per turn rather than spliced into
        // `static_prompt_body`, so without this they would be always-loaded
        // text sitting outside the only meter that watches it.
        //
        // The build state is billed at its WORST case, like the body above.
        let engine_build = ENGINE_BUILD_POINTER.chars().count()
            + BUILD_STATES
                .into_iter()
                .map(|(update, behind, wedged)| {
                    engine_build_block(&version_status(update, behind, wedged))
                        .chars()
                        .count()
                })
                .max()
                .expect("four states");

        // The URL itself is a runtime reading rather than engine-authored
        // text, so only the pointer and the block's delimiters are billed.
        let client_url = CLIENT_URL_POINTER.chars().count() + client_url_block("").chars().count();

        // The mode's rule block and note guidance, billed at the same
        // worst-case rule as the tool schemas above. It reaches only a
        // workspace running the mode, and this meter measures the
        // engine-authored surface rather than one workspace's prompt.
        let context_mode = super::super::context_mode::system_prompt_section(
            super::super::context_mode::ContextMode::On,
            super::super::context_mode::SweepSchedule::default(),
        )
        .chars()
        .count();

        vec![
            ("static prompt body + rule consts", body),
            ("flat tool schemas, JSON wire form", flat),
            ("grouped manifest tool schemas, JSON wire form", grouped),
            ("System Knowhow routing list", knowhow),
            (
                "experimental context mode rule + note guidance",
                context_mode,
            ),
            (
                "ENGINE BUILD (dev): pointer + widest tail block",
                engine_build,
            ),
            ("client URL: pointer + tail block delimiters", client_url),
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
    /// schema hide behind twenty lean ones. Same reasoning as
    /// `system_knowhow_descriptions_stay_routing_sized`, which caps each
    /// frontmatter `description` rather than their sum.
    ///
    /// Most tools clear this by a wide margin. The few that do not are
    /// enumerated in [`PER_TOOL_CEILING_EXCEPTIONS`] with a reason each, so one
    /// more is a deliberate act rather than a drift.
    ///
    /// The number is chosen against the FROZEN CONTRACT rather than picked
    /// round: strip every `description` from a schema (what
    /// `print_frozen_tool_contract` dumps) and what remains is the part a trim
    /// may not touch. For all but the exceptions that floor is a few hundred
    /// characters, so 1,500 leaves room for prose without inviting any.
    const PER_TOOL_SCHEMA_CEILING_CHARS: usize = 1_500;

    /// The tools allowed past [`PER_TOOL_SCHEMA_CEILING_CHARS`], each with the
    /// reason its schema is structurally larger than the rest.
    ///
    /// Every value is a RATCHET set just above where the schema sits, so
    /// growing one of these is as deliberate as adding a row. A reason that is
    /// only "it is long" does not belong here: the test is what stops a schema
    /// re-accumulating the prose the prompt and the knowhow already own.
    ///
    /// Each reason names the STRUCTURE that cannot shrink.
    /// `print_frozen_tool_contract` dumps the floor behind it. Where a ceiling
    /// sits far above that floor, the gap is prose that other tests pin phrase
    /// by phrase, and the reason enumerates it.
    const PER_TOOL_CEILING_EXCEPTIONS: &[(&str, usize, &str)] = &[
        (
            "await_event",
            2_845,
            "four tests pin thirteen distinct phrases on it (forward-only plus \
             the arming race, spent-and-resubscribe, the consecutive cap, the \
             own-child carve-out), each one a failure that reached a user, and \
             `condition` carries the operator set that makes a filter valid. \
             Raised from 2,550 by ADR 0113: `event_type` now names the three \
             families the gate admits and the transient frames it refuses, \
             because the wait it grants is the one surface that says so before \
             a name is spent. Raised from 2,700 by ADR 0119: `condition` now \
             states the field-path rule and three more operators, and both are \
             chosen while writing the call, where no knowhow load can reach",
        ),
        (
            "run_coding_agent",
            2_850,
            "eleven parameters; the source-checkout precondition is pinned in \
             BOTH the description and `folder` because a packaged install's \
             agent believed the unconditional wording and narrated a spawn the \
             engine refuses, and the spawn-ack-is-not-a-result rule stays \
             inline rather than behind the knowhow pointer because it fires at \
             spawn time, before anything would be loaded. Raised from 2,600 by \
             `model`'s own JSON-schema enum: 317 chars of frozen shape, the \
             union of both backends' model ids, replacing the examples its \
             description used to carry",
        ),
        (
            "triggers",
            2_980,
            "seven actions, each contributing its own summary line and its own \
             `(requires: …)` clause, plus the create schema's union shapes for \
             `cron` and `on`. system-knowhow/triggers.md deliberately does NOT \
             restate the cron format, it defers to this schema, so the format \
             spec cannot move out. Raised from 2,550 by the *trigger model*: \
             `model` and `reasoning_effort` add 202 characters of frozen shape \
             before a word of prose (a null-union each, and the six-value \
             effort enum), and they are declared once, on create, because the \
             union across operations is first-wins. Raised from 2,950 by ADR \
             0119: `on` carries the condition operator set, so three new names \
             and the field-path rule land here rather than in the knowhow",
        ),
        (
            "navigate_ui",
            2_300,
            "the two frozen enums the SDK is generated from (17 targets, 18 \
             settings views) are 758 chars before a word of prose; the settings \
             gloss is routing information available on no other surface, and \
             it names only the views whose own value does not (`mcp` is not \
             glossed, because the value already says what the page is)",
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
            "edit_file",
            2_000,
            "two modes that share one property bag, so `json_path` / `new_value` \
             and `old_string` / `new_string` are both always present and each \
             must say which mode it belongs to. On top of that frozen shape, \
             ADR 0093 added a second TARGET: `repo` and `commit` are 424 chars \
             carrying the one thing a reader cannot infer, that a repo edit \
             writes a checkout the user has open and records nothing. The \
             refusal wording is pinned by a test, and `commit` has to name \
             run_coding_agent, since a reviewable change is the alternative it \
             is being chosen over",
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
            1_960,
            "two properties added to a domain already at 1,485. `thread_id` is \
             the read half of finding a past conversation, and it has to say \
             which `event_type` to pair it with or the filter returns the \
             whole transcript including every streamed token. `event_id` is \
             ADR 0085's dereference, and it has to state the `evt-<32 hex>` \
             form, because a pointer the model cannot spell back is a pointer \
             it will not use. It also has to say that a call's address returns \
             the pair: a lookup that answers with two rows and never said so \
             reads as a bug. Raised from 1,860 by the `event_types` action, a \
             fourth action and so a fourth summary line: it is where a \
             subscription's refusal sends the caller, and no other surface \
             answers which names exist",
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

    /// Diagnostic dump, not an assertion: what a workspace is actually SENT,
    /// at each gate setting.
    ///
    /// The sibling ranking below measures the engine-authored surface, which
    /// is deliberately gate-blind. This one answers the other question, the
    /// one ADR 0088 priced: how many characters does THIS workspace pay for.
    /// Note it sums the schema objects only, so the array's own brackets and
    /// separators (73 characters at 72 tools) are not in the figure.
    ///
    ///   cargo test -p lucidos-engine --lib print_gated_array_sizes -- --ignored --nocapture
    #[test]
    #[ignore]
    fn print_gated_array_sizes() {
        let wire = |caps: &ToolCapabilities| {
            let tools: Vec<_> = crate::llm::tools::get_default_tools(caps)
                .into_iter()
                .chain([crate::llm::tools::get_notification_tool()])
                .chain(crate::capability_manifest::llm_tools())
                .chain(crate::llm::tools::chat_tail_tools(caps))
                .collect();
            let chars: usize = tools.iter().map(wire_chars).sum();
            (tools.len(), chars)
        };
        for (label, caps) in [
            (
                "every gate open, the mode off",
                ToolCapabilities::all_open(),
            ),
            (
                "no email account, no intent, image provider on",
                ToolCapabilities {
                    email_account: false,
                    intent: false,
                    image_provider: true,
                    context_mode: false,
                },
            ),
            ("no capability at all", ToolCapabilities::default()),
        ] {
            let (n, chars) = wire(&caps);
            println!("{label}: {n} schemas, {chars} wire chars");
        }
    }

    /// Diagnostic dump, not an assertion: every tool schema ranked by wire
    /// cost, both areas, no truncation.
    ///
    /// `always_loaded_context_stays_under_budget` prints only the top ten,
    /// which spots a regression but hides the tail: twenty 1,200-char schemas
    /// are another 24k and none of them reaches that list. Run before and after
    /// a trim to record per-tool deltas.
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

    /// Makes a prompt-budget regression visible at `cargo test` rather than in
    /// a token bill.
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
    /// prompt body AND every tool schema, so a pointer added later is covered
    /// the moment it is written. Schemas are in scope because they route too.
    ///
    /// Ids are filename-derived, so a knowhow rename is what breaks them.
    #[test]
    fn every_knowhow_id_the_resident_context_routes_to_resolves() {
        let repo = crate::paths::repo_root().expect("repo root resolves under cargo test");

        let mut haystack = format!(
            "{}{}{}",
            static_prompt_body(true, 500, ContextMode::Off),
            static_prompt_body(false, 500, ContextMode::Off),
            static_prompt_body(true, 500, ContextMode::On)
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

    /// The prompt never orders a tool call the mode has withdrawn.
    ///
    /// `todo_write` is absent from the mode-on tools array, so the base body's
    /// standing "call it at the start" instruction was unfollowable on every
    /// mode-on turn. The list lives under a `[TODO]` heading instead.
    #[test]
    fn the_mode_on_body_points_at_the_todo_heading_rather_than_the_tool() {
        let off = static_prompt_body(false, 500, ContextMode::Off);
        let on = static_prompt_body(false, 500, ContextMode::On);

        assert!(
            off.contains("todo_write"),
            "the tool exists whenever the mode is off, so the prompt names it"
        );
        assert!(
            !on.contains("todo_write"),
            "the mode withdraws the tool, so nothing may tell the model to call it"
        );
        assert!(
            on.contains("[TODO]"),
            "the mode-on body has to say where the list goes instead"
        );
    }

    /// A direct child's completion is the one state change nobody has to
    /// subscribe to: the ADR 0011 fan-in re-opens the parent with the child's
    /// result whether or not a wait exists. Subscribing to it anyway spends a
    /// subscription and arms a timeout for a wake the thread gets regardless.
    ///
    /// **The carve-out is a redundancy, not a limit, and the section has to say
    /// so in that order.** Worded as a bare "not for a thread you spawned" it
    /// generalises into "cross-thread waits are unreliable", which is false:
    /// matching is workspace-wide (see
    /// `a_wait_matches_a_child_completion_belonging_to_another_thread`). So the
    /// positive claim leads and the exclusion follows it.
    ///
    /// **The condition it names has to be one that exists.** `child_thread_id`
    /// is a field on `ChildThreadCompleted`, which only the parent/child fan-in
    /// emits, so scoping someone else's coding-agent session with it matches
    /// nothing and the wait never fires. `thread_id` is the field that exists,
    /// on every thread event, supplied by the *matchable payload*
    /// (`a_wait_scopes_to_one_coding_agent_session_on_every_path`).
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

        // The defect: a recipe naming a condition the event does not carry.
        assert!(
            section.contains("thread_id"),
            "the section must name the condition that scopes a wait to one \
             thread, or 'watch someone else's session' has no mechanism:\n{section}"
        );
        assert!(
            !section.contains("child_thread_id"),
            "`child_thread_id` is a field on ChildThreadCompleted only, which the \
             parent/child fan-in alone emits. Naming it here sends a thread to \
             scope a coding-agent session with a condition that matches nothing, \
             and the wait then never fires:\n{section}"
        );

        // A set discovered at wait time, and the trap inside it.
        assert!(
            section.contains("RE-CHECK ON EVERY WAKE"),
            "one subscription is spent by one event, so waiting on everything \
             currently running is a loop: without the re-check the thread reports \
             'all done' when the FIRST agent finished:\n{section}"
        );
        assert!(
            section.contains("status [\"running\"]"),
            "the set has to be discovered before it can be waited on, and the \
             section must name the filter that lists it:\n{section}"
        );

        // A condition read as a green light. "Release the patch when the coding
        // agents are done, auto approved" was acted on immediately.
        assert!(
            section.contains("PRECONDITION, NOT A GO-AHEAD"),
            "a 'when X finishes' clause is something to wait for; a \
             pre-approval attached to it covers the action once X has happened \
             rather than replacing X:\n{section}"
        );
    }

    /// A trigger was the one first-class thing the agent names in a reply with
    /// no link form of its own. So it linked the Triggers PANEL, and when told
    /// to link the trigger it invented `trigger:<id>`, which nothing claimed.
    /// The section has to carry the form AND keep the two destinations apart.
    ///
    /// Source-scanned and scoped to the section, like
    /// `the_state_change_section_excludes_the_threads_own_child`: this is plain
    /// text inside [`SYSTEM_PROMPT_BASE`], and the scoping keeps this test's own
    /// prose from satisfying its assertions.
    #[test]
    fn the_file_references_section_links_a_trigger_not_just_the_panel() {
        let repo = crate::paths::repo_root().expect("repo root resolves under cargo test");
        let path = repo.join("crates/lucidos-engine/src/engine/chat/process/system_prompt.rs");
        let src = std::fs::read_to_string(&path)
            .unwrap_or_else(|e| panic!("read {}: {e}", path.display()));

        // The heading, not the cross-reference to it in NAMES_NOT_IDS_RULE.
        let from = src
            .find("FILE REFERENCES:\n")
            .expect("the prompt no longer has a FILE REFERENCES section");
        let section = &src[from..];
        let section = &section[..section.find("\n\n").unwrap_or(section.len())];

        assert!(
            section.contains("(trigger:<id>)"),
            "without the link form the agent has nothing to write but the panel \
             link, which is the reported bug:\n{section}"
        );
        assert!(
            section.contains("`triggers` action 'list'"),
            "a link form the agent cannot fill in is not a link form: the \
             section must say where the id comes from:\n{section}"
        );
        assert!(
            section.contains("[Triggers](triggers)` is the LIST"),
            "both links exist and mean different things, so the section must \
             say which is which or the panel link keeps winning:\n{section}"
        );
        // Says WHY the trigger link is worth writing: the row carries the
        // actions. Drop this and the two links read as interchangeable.
        assert!(
            section.contains("Run once"),
            "the section must name what the trigger link lands on:\n{section}"
        );
    }

    /// Scoped to "Before creating", the block leaves "run my digest trigger
    /// now" with no route. The agent then guesses and resumes a paused trigger,
    /// which only restores the schedule and runs nothing. Narrowing the trigger
    /// row back to create/update reopens exactly that gap.
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
    /// silently invalidates them.
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
    /// only fires if the prompt says something the route recognises. Reading
    /// the real sources is what makes this a check on the set rather than three
    /// assertions that the same string equals itself.
    ///
    /// The knowhow's frontmatter `description` is the third arm because it is
    /// the SECOND discovery path: retrieval, for a user who types the request
    /// rather than pressing the button. A description that no longer mentions
    /// the sentence leaves the feature alive on the hardcoded route alone.
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

    /// On an install with no source checkout the prompt must say so outright.
    /// Otherwise the agent claims to have read the source, spawns a coding
    /// agent, and tells the user to Apply and rebuild.
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
    /// `run_coding_agent(workspace="…")` returns in
    /// `agentic_loop_special_tool` before the local source guard, and the
    /// TARGET engine applies its own check, so it stays valid here. An
    /// unqualified "never spawn a coding agent for a Lucidos change" would make
    /// that capability unreachable from every packaged install.
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

    /// Exactly two variants, and they genuinely differ. A refactor that
    /// collapsed them would silently restore the bug on packaged installs.
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
    /// the engine rebuild/restart choreography is dev-only, because a packaged
    /// install never rebuilds from source.
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

    /// A prompt that pre-writes a conclusion invites the model to reach for the
    /// words rather than run the check. So no wording for an unverified outcome
    /// may live here.
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

    /// The rule must state an ORDER. Phrased as a review step, it fires once
    /// the model has decided what it thinks, which is precisely when it will
    /// not go and check.
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

    /// The prompt's WORKSPACE line must never carry a `$HOME`-rooted absolute
    /// path. Runs against the REAL `$HOME`, read-only, so it is safe under the
    /// parallel test runner and fails on the very machine that would leak.
    #[test]
    fn workspace_line_abbreviates_home_and_leaks_no_absolute_home_path() {
        let home = std::env::var("HOME").expect("HOME must be set to run this test");
        let workspace = PathBuf::from(&home).join("workspaces/myws");

        // The real timezone section, so the splice is checked against what
        // ships rather than a stand-in.
        let timezone = super::super::turn_clock::timezone_section("Europe/Oslo");
        let section =
            workspace_identity_section("myws", &workspace, &timezone, "USER LANGUAGE: English");

        // Still identified by name and by the shape of its path, so the agent
        // loses no usable context.
        assert!(section.contains("WORKSPACE: myws (~/workspaces/myws)"));
        // The home dir's own name never reaches the model provider.
        assert!(
            !section.contains(&home),
            "identity section still carries the home-rooted absolute path {home}:\n{section}"
        );

        // The surrounding sections are still spliced in.
        assert!(section.contains("USER TIMEZONE: Europe/Oslo"));
        assert!(section.contains("TIMEZONE HANDLING:"));
        assert!(section.contains("USER LANGUAGE: English"));
        assert!(section.contains("PERSONAL DATA ACCESS:"));

        // The reading itself moved to the message tail, and the identity
        // section is the front of a cached tier: a clock here rewrites all of
        // it every turn. `turn_clock` owns the rule.
        assert!(
            !section.contains("CURRENT TIME:"),
            "the cached identity section must carry no clock reading:\n{section}"
        );
    }

    /// Nothing the user can see is labelled with a change id, so a question
    /// that quotes one is unanswerable. The rule must ban raw ids in prose,
    /// questions and option labels, say what to use instead, and keep ids legal
    /// in tool arguments.
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
        // A trigger link is the case the carve-out was reported missing on: the
        // agent read the ban as covering the id in the target and linked the
        // Triggers PANEL instead of the trigger. The exemption must name the
        // form and say which half of the link the name goes in.
        assert!(
            rule.contains("(trigger:<id>)") && rule.contains("The NAME goes in the label"),
            "rule must exempt the trigger link form and place the name in the label:\n{rule}"
        );
    }

    /// The rule has to carry two things: the prohibition, and what to do
    /// instead. An agent that only knows it may not work around a tool still
    /// has to be told that reporting the block IS the correct turn.
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

    /// Lucidos has no text-entry option, so tapping an "Other, I'll type it"
    /// button returns that label as the user's answer. The rule must ban the
    /// option, explain WHY, and name both real escapes. Otherwise the agent
    /// keeps inventing the button to fill a gap that does not exist.
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

    /// Abbreviation is a `$HOME`-prefix collapse, not a blanket redaction: a
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
