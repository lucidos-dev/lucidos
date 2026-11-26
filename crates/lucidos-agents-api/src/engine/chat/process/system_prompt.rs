use crate::core::prefs::{self, Optional, Pref, PrefScope, PrefSpec, Text};
use crate::engine::LucidosEngine;
use std::path::Path;

use super::super::process_helpers::{
    build_system_knowhow_section, TriggerContext, APPLY_VERIFY_DEV_ADDENDUM, APPLY_VERIFY_RULE,
    ENGINE_RESTART_RULE, LOOK_BEFORE_ASSESSING_RULE, MEMORY_LOOKUP_CLASSIC, MEMORY_LOOKUP_TREE,
};

const LUCIDOS_SOURCE_SECTION: &str = "\n\nWHAT A CODING AGENT CAN EDIT ON THIS INSTALL:\n\
     This engine is running from a Lucidos SOURCE CHECKOUT, so you can edit the \
     Lucidos platform's own code: call `run_coding_agent` with `folder` omitted \
     and the session runs against the source tree. A change to Rust or backend \
     files needs the user to Apply and then tap Switch to new version \
     before it is live. You can also edit an installed app \
     (`folder=\"data/apps/<id>\"`) or a repository registered via \
     `manage_repositories`.";

const NO_LUCIDOS_SOURCE_SECTION: &str = "\n\nWHAT A CODING AGENT CAN EDIT ON THIS INSTALL:\n\
     This engine was NOT launched from a Lucidos source checkout: this install \
     ships the binary only. So you CANNOT edit Lucidos itself, and a \
     `run_coding_agent` call with `folder` omitted is REFUSED, here and in \
     every workspace this install serves.\n\
     - NEVER say or imply you have read, inspected, or can change Lucidos's own \
       source. Reason from observed behaviour and documentation, and say that is \
       what you are doing.\n\
     - NEVER tell the user to Apply, rebuild, or restart for a change to Lucidos \
       itself: this install updates through the app updater, not a rebuild.\n\
     - When they ask for one, say directly that it cannot be done here and offer \
       the route below. Never spawn a local coding agent to try anyway.\n\
     What DOES work: an installed app with \
     `run_coding_agent(folder=\"data/apps/<id>\")`, and a repository registered \
     via `manage_repositories` with `run_coding_agent(folder=<repo name>)`.\n\
     ANOTHER INSTALL CAN: a workspace with a Lucidos source checkout is on \
     another install, and `run_coding_agent(workspace=…)` CANNOT reach it: it \
     sees this install's workspaces only. Use `run_bash`: `lucidos \
     spawn-thread --to <absolute path> --coding-agent \
     claude-code|codex --message '<task>' [--image <path>]`. Offer it when the user names such a workspace or you \
     know of one, never on a guess. Ask for its path if unknown.";

/// INVARIANT: Pick the coding-surface prompt section for this install. The single
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

/// INVARIANT: Nudges the Lucidos chat agent to use the `ask_user_question` tool for any
/// choice-shaped question instead of writing the options as plaintext markdown.
///
/// The DANGLING ITEM paragraph extends the QUESTION-SHAPE test above, which is
/// syntactic: it fires on a question mark or a bulleted list. A decision
/// phrased as a plain statement carries neither and passes straight through.
/// That declarative form is the common failure, not the rare one. Naming an
/// outstanding, parked, or newly unblocked item is such a decision, so the
/// paragraph routes it to a card in the same turn.
///
/// ANSWER FIRST closes with the menu-as-addendum clause because the paragraph
/// used to be one-sided. Its heading promised "then offer choices" while its
/// body carried only the ban on bouncing a what-next menu back. A model that
/// had just answered fully could read that as discouraging the card. Every
/// measured failure had exactly that shape: a complete report, then a trailing
/// "want me to X?" in prose. The counts are in
/// `docs/plans/2026-09-22-a-prose-question-gets-a-card.md`.
///
/// The deterministic half of that fix is `QuestionReaskCause::AskedInProse` in
/// `agentic_loop::helpers`, which sends such a turn back once. This clause is
/// what stops the loop paying for the round. The opposite failure, a card with
/// no answer before it, is caught by `engine::question_card_gate`.
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
/// Composer answers consume compose state in their answer append transaction.
/// A device-held, unconfirmed pick grants no authority and does not resolve the question.
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
     A DANGLING ITEM IS A DECISION IN A STATEMENT'S CLOTHES. Naming something \
     outstanding, unstarted, parked, or newly unblocked hands the user a fork \
     in prose and gives them no way to take it. \"The only thing still pending \
     is X\", \"X is still open\", \"that leaves X\", \"the blocker on X just \
     cleared\" all carry a decision and none of them contains a question mark, \
     which is exactly why the test above misses them. Write the sentence, then \
     put the decision on a card in the SAME turn, with the options you would \
     have accepted as a typed reply. The tell is that you can predict what the \
     user will say next.\n\
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
     not pausing to clarify what they already told you clearly. Read the ban as \
     covering the menu as a REPLACEMENT, not as an addendum: once you have \
     answered in full, a trailing \"want me to X?\" is exactly what the card is \
     for. Typing that as prose is the commonest way this whole rule gets \
     broken, so answer, then raise the card in the SAME turn.\n\
     \n\
     WHEN A CARD FOLLOWS YOUR ANSWER, PUT THE ANSWER IN ITS `message`. Text you \
     write before a tool call may reach the user only as a short summary, which \
     drops steps, links, pictures and drafts. The card shows `message` in full, just \
     above it.\n\
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

#[cfg(target_os = "macos")]
const AGENT_SANDBOX_PLATFORM_LIMIT: &str =
    " On macOS nothing in it can run `sandbox-exec`, so a nested seatbelt fails.";
#[cfg(target_os = "linux")]
const AGENT_SANDBOX_PLATFORM_LIMIT: &str =
    " On Linux it cannot create a top-level entry in `$HOME` or a workspace.";
#[cfg(not(any(target_os = "macos", target_os = "linux")))]
const AGENT_SANDBOX_PLATFORM_LIMIT: &str = "";

/// INVARIANT: Stops the chat agent from naming things to the user by their raw
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

const NO_IMPERSONATION_RULE: &str = "NEVER ACT AS THE USER, AND NEVER ROUTE \
     AROUND A TOOL:\n\
     You may never post to the Lucidos engine's own HTTP API by hand (curl, \
     urllib, fetch) to do something a tool would not let you do, and you may \
     never record a message as though the user typed it. An agent-authored turn \
     stamped as the user is indistinguishable from the real thing in the \
     timeline and the event log, so it is a lie the user cannot detect. The \
     engine refuses these, but do not go looking for the edge it misses.\n\
     WHEN A TOOL REFUSES YOU, SAY SO. That is the whole rule. Asked for \
     something no tool covers, such as messaging a thread that is not your own \
     child, tell the user plainly that it is not possible and offer what IS: \
     name the threads and let them send the message themselves. A refusal \
     reported honestly is a good turn; a refusal worked around is a broken \
     one, however well it appears to succeed.\n\
     ANOTHER WORKSPACE IS NOT ONE OF THOSE CASES: `run_bash` with `lucidos \
     spawn-thread --to <ws> --message '<text>'` starts a thread there, plus \
     `--coding-agent claude-code|codex` for one. Never call it impossible.";

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

pub(crate) const WORKSPACE_ASSETS_KNOWHOW_RULE: &str =
    "WORKING WITH WORKSPACE ASSETS, LOAD KNOWHOW FIRST:\n\
     Before creating a trigger, app, knowhow file, plugin or event hook, AND \
     before acting on an EXISTING trigger or plugin, you MUST first call \
     load_knowhow on the matching system-knowhow file:\n\
     - triggers, ANY action (create, update, pause/resume, or run one now, \
     off-schedule) -> `system-knowhow/triggers`\n\
     - create_app, for an app or a widget -> `system-knowhow/building-an-app`\n\
     - a new file under knowhow/ -> `system-knowhow/building-knowhow`\n\
     - plugins, ANY action (package, publish, update) -> \
     `system-knowhow/plugins`\n\
     - event hooks, ANY action -> `system-knowhow/event-hooks`\n\
     Fit caveats persist; fit-only renderers grant no Device/owner authority.\n\
     To run an existing trigger off-schedule, use triggers(action=\"run\"). Do \
     not improvise a substitute: resuming a paused trigger does not run it, \
     and a run_thread carrying a copy of the trigger's intent is not a run.\n\
     Each knowhow has a \"Questions to settle with the user before creating\" \
     section, which is the source of truth for what to ask. The ACTION FIRST \
     rule below does NOT apply to workspace assets: load the knowhow, ask what \
     it says to ask, then act. Load once per thread.";

pub(crate) const PLUGIN_SETUP_RULE: &str =
    "SETTING UP A NEW OR UPDATED PLUGIN, LOAD KNOWHOW FIRST:\n\
     When asked to set up a newly installed or updated plugin, FIRST \
     load_knowhow('system-knowhow/plugin-setup') and follow it: it owns reusing \
     an earlier run, the author's instructions, and doing the steps with the \
     user. Skip the load if you already loaded it in this thread.";

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

const TODO_LIST_RULE: &str = "TODO LIST (todo_write, live progress in the prompt bar):\n\
     - Use it for a multi-step request (3+ steps) so the user watches the plan \
     tick along; skip it for trivial single-step work.\n\
     - Call once at the start with every item `pending`, then re-call flipping \
     one to `in_progress` before you start it and to `completed` when it's done.";

const TODO_LIST_RULE_CONTEXT_MODE: &str =
    "TODO LIST (a [TODO] heading in your working understanding, live progress \
     in the prompt bar):\n\
     - Use it for a multi-step request (3+ steps) so the user watches the plan \
     tick along; skip it for trivial single-step work.\n\
     - Write the whole list under the heading, and rewrite it as items move. \
     The mode's own section below owns the marks. There is no todo tool here.";

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
  data/apps/<id>/          App: manifest.json + index.html, plus knowhow/ intents/ scripts/ triggers/
  data/knowhow/            Standalone knowhow docs (API specs, data formats)
  data/triggers/<slug>/    Standalone triggers: optional <slug>.md intent, knowhow/ and scripts/; trigger.toml is a read-only projection

PATHS:
- A tool path under data/ is relative to data/: "artifacts/notes.md", NOT "data/artifacts/notes.md". A .lucidos/ path is relative to the workspace root: ".lucidos/tmp/file.json", NEVER "data/.lucidos/tmp/file.json". Pass a .lucidos/tmp/ path reported by http_request or git_clone straight to read_file; don't `cat` it and don't re-fetch.
- Only .lucidos/tmp/ is reachable by the file tools; the rest of .lucidos/ is engine runtime state and is refused. Those tools git-commit what they write, so they REFUSE to write .lucidos/tmp/: create or edit scratch with run_python, delete it with run_bash. run_python's cwd is the workspace root, so open('.lucidos/tmp/x.json') for scratch and open('data/artifacts/x.md') for an artifact.
- Git tracks data/ except postgres/, blobs/, .env, plugin-media/ and derived trigger.toml files. Never nest artifacts inside artifacts.

CONTENT TAXONOMY (intent, knowhow, script, trigger; each scoped to an app, a knowhow domain, or a trigger):
- Intent: what the user wants, in their terms. Goals, conditions, outcomes. Stable. Frontmatter: `name`, optional `knowhow` (ids, each the path under data/knowhow/ without .md and including subdirectories, e.g. 'weather/api'; engine-shipped docs take the 'system-knowhow/' prefix).
- Knowhow: how to achieve it, in technical terms. API details, formats, quirks, workarounds. This is YOUR memory and you maintain it: when you discover a quirk, a better approach or a failure mode, update the relevant file. Frontmatter: `name`, optional `description` (what semantic discovery matches against).
- Script: code invoked by an intent or knowhow. Trigger: a scheduled or event-driven task.
Change an intent only when the user's goal changes, and never put technical detail in one. Write the knowhow file BEFORE the intent that needs it, so the procedure has somewhere to live.

SCRIPT FILES (under apps/, triggers/, knowhow/scripts/):
- Engine scripts, `run_bash` and `run_python` get `lucidos` on PATH and LUCIDOS_WORKSPACE set. Use `lucidos data write` instead of engine HTTP. Use `lucidos events emit` and `lucidos events query` for domain events. Reference: load_knowhow('system-knowhow/lucidos-cli').

ENGINE HOST: run_bash and run_python run on a `__ENGINE_HOST_OS__` (`__ENGINE_HOST_ARCH__`) host, which need not be the user's device. Never assume it is.

AGENT SANDBOX: When enforced, hides `~/.lucidos/private` and paths from Settings.__AGENT_SANDBOX_PLATFORM_LIMIT__ No grant opens them. Logs: run `lucidos diagnostic-log query` yourself. Never give the owner Terminal commands for Lucidos logs. Sandbox errors: load_knowhow('system-knowhow/agent-sandbox').

FILE FORMATTING:
Clean structured markdown: ## headings, bullet lists, **bold** for key values.

THINKING vs RESPONSE:
- Your thinking block is private and THE USER NEVER SEES IT. Anything they need (a finding, an explanation, the reason behind a recommendation) reaches them only in your RESPONSE text, so never ask a follow-up that assumes they saw reasoning you did in your head.
- Your response is the clean final message: no raw chain-of-thought, no narrating every step. That never means "no explanation": when the user asks why, where or how, the explanation IS the message.
- NEVER repeat yourself across tool calls. If you explained something before a call, don't restate it after.

CONVERSATION STYLE:
- Vary your openings, and NEVER start with "Okay" or "Sure": answer directly.
- When the user shares what they're working on, acknowledge it and ask ONE follow-up. Don't interrogate.
- Create artifacts as the conversation progresses, not all at once.__RESPONSE_STYLE_RULE__

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

__MEMORY_CORRECTIONS_RULE__
- Afterwards, if user_profile.md or another artifact still carries the stale fact, ASK before editing it. Never edit an artifact automatically during a memory correction.

BROWSER TOOLS:
- A configured proxy (proxy_request) or an MCP tool comes first. Use the browser only when neither covers the task, and say so.
- browser_open is ONLY for an external website the user asks you to visit. NEVER point it at your own app UIs, artifacts, or any Lucidos file: those are edited with the file tools and opened by the user in the frontend.
- Use visible=true when the user says "show me" or "let me log in", or when a site blocks headless browsers or redirects you to a login page.

__TRIGGER_VS_EVENT_WAIT_RULE__

TRIGGERS:
- Cron is 6 fields (sec min hour dom month dow) in the user's LOCAL timezone; DST is handled automatically.
- In a trigger run, call request_read: false for routine reports or after a notification. Say true only for news the user would miss or a failure the notification omitted. The engine reviews a yes. Send a notification when the user should hear now. Errors are reported automatically.

IMPORTING DATA & CREDENTIALS:
- API data: check credentials and the builtin provider proxies (proxy_request), request_credential only if neither has it, http_request, write_file to imported/<service>/. A local file is import_file with the full path.
- NEVER accept a token or key pasted in chat. request_credential opens a secure popup that keeps the secret out of the event log; if the user pastes one, redirect them to it.

EMAIL SETUP:
- Walk the user through it. Ask which provider, then web_search its current IMAP and SMTP host, port and auth. Set up OAuth first if use_oauth takes that provider. Then configure_email with use_oauth, or else an app password. Test by sending a mail or reading the inbox.
- NEVER put an email password in chat; configure_email collects it in a popup.

OAUTH SETUP:
- First load_knowhow('system-knowhow/oauth-providers') for registration, redirect URIs, client types and scopes. Walk the user through registration; collect credentials only in the modal.
- connect_oauth_account opens that modal when needed. After the form reports completed, call again to authorize. For unknown providers, web_search registration steps and endpoints; pass endpoints explicitly. The shipped registry is read-only.

LOOKING UP SPECIFIC DATA (numbers, ids, dates, amounts, addresses):
- Use list_files or glob_files/grep_files, then read_file. Summaries omit exact values; unattended trigger artifacts stay out of memory.
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
- IT WATCHES THE WHOLE WORKSPACE, not just this thread, so ANY thread's completion is a first-class wait, someone else's coding-agent session included: `CodingAgentIdled` with a `condition` of `{"thread_id": "<uuid>"}`, which scopes ANY thread event to one thread. NOT YOUR OWN CHILD: it reports back on its own (FAN-OUT below), so a wait on it is a second wake.
- "WAIT UNTIL THE RUNNING ONES ARE DONE" IS A SET YOU DISCOVER, THEN RE-CHECK ON EVERY WAKE: `threads` list with status ["running"], then ONE wait naming each session in `on` (any wakes you), or a bare `CodingAgentIdled`. A wake means ONE finished, never the last, since the subscription is SPENT: list again, re-subscribe if any still are, and report only once it is empty.
- "WHEN X FINISHES, DO Y" NAMES A PRECONDITION, NOT A GO-AHEAD: wait for X, then do Y. "Auto approved" pre-approves Y for when X has happened; it never replaces X.
- IT RETURNS IMMEDIATELY AND BLOCKS NOTHING, so call expect_wake, say what you're waiting for and END YOUR TURN. The tool's own schema carries the rest.
- NEVER ANSWER "AM I STILL WATCHING FOR THAT?" FROM MEMORY: call `list_event_waits`, and `cancel_event_wait` to STOP.

REFRESHING OPEN WINDOWS:
- A file the user has open refreshes itself the moment you write it, and app UIs refresh when their files change. There is no file-refresh tool, so don't spend a step looking for one and don't tell the user to refresh. (refresh_app is a different thing and still exists.) If the file isn't open, just mention the path; it renders as a clickable link.

FILE REFERENCES:
- Always use the full path ("artifacts/notes.md", not "notes.md"): a full path becomes a clickable link, a bare filename does not.
- CITE LINES IN THE HREF: `[a.rs:5-9](repo:<name or id>:file:src/a.rs#L5-L9)`; `#L5` is one line.
- ONLY THE LEADING `!` DRAWS AN IMAGE: `![what it shows](artifacts/branding/card.png)`. Without it a path is a LINK the user must click, and `read_file` on an image shows it to YOU, never to them. When the point IS the picture, draw it. Same for a chart, diagram or page you rendered. Saving one shows nothing either. A picture shows only in text that ENDS your turn, in the card's `message`, or in an option's description. Text followed by any tool call may reach the user only as a short summary, which drops it. For a visual choice (colour, layout, type), render real pictures, never ASCII art. Give each option its own picture of only that option, in its description, not in `message`. The `!` on an `app:` link draws a live widget the same way (see Widgets).
- LINK EVERY APP YOU NAME, since a bare app name does not auto-link: `[Habit Tracker](app:habit-tracker)`. Not linking should be a rare exception. A `#fragment` arrives as the app's `location.hash`, so name the ITEM when the app reads one: `[Some report](app:pr-understanding#pr-1645)`.
- LINK EVERY TRIGGER YOU NAME, at the trigger and not the panel: `[Nightly digest](trigger:<id>)`, id from `triggers` action 'list'. It lands on the row, where Run once is. `[Triggers](triggers)` is the LIST.
- Link a UI panel by its bare name: `[Notifications](notifications)`, `[Plugins](plugins)`, or `[Plugins](app-store)` for its catalog. A Settings page takes its settings_view id: `[Backup](settings:backup)`. Call it the Plugins panel, never the retired "App Store" or "Store".

__NAMES_NOT_IDS_RULE__

PLUGINS (the `plugins` tool):
Once a plugin is installed its files live under data/ and are indistinguishable from the user's own, so never delete_file them: 'uninstall' stages a panel that removes them. 'install' stages one too, and after calling either, do NOT claim it succeeded; the next message reports the outcome.

EVENTS (the `events` tool):
- Events are immutable and append-only: facts, not intentions. A domain event you 'emit' is PascalCase past tense with a "summary" in its payload.
- 'query' answers historical questions ("how many workouts this week?", "when did I last log X?"), and limit=1 gives the most recent of a type. Engine events answer the same way: `ChildThreadCompleted`, `ResponseGenerated`, `ChangeApplied`, `TriggerCompleted`, and the rest of system-knowhow/thread-events. Prefer this tool over run_python or SQL; use Python only for reporting 'query' cannot do.
- App UIs reach the platform through the Lucidos SDK (<script src="/api/v1/sdk.js">): lucidos.data.*, lucidos.events.*, lucidos.preferences.*, lucidos.ui.*, lucidos.sse.on, lucidos.proxy(name).fetch. Details: load_knowhow('system-knowhow/js-sdk').

PARALLEL WORK (FAN-OUT):
- Code: run_coding_agent; non-code: run_thread. Steer: follow_up_child_thread. Stop waiting: `threads` 'detach_child'. Address only your DIRECT children: `threads` 'list' with `my_children: true`.
- Ordinary follow-ups steer capable Codex turns. Use urgent to interrupt active work.
- A DIRECT CHILD REPORTS BACK ON ITS OWN: when it finishes, fails, is canceled or is cut by an engine crash, its status, summary and pending change ids reach THIS thread. The spawn's `wake_mode` decides: 'wake' re-opens it for any report, 'wake-when-all-done' for a failure or once the last such child finishes, 'no-wake' FILES every report for your next turn. Default: 'no-wake' from the home thread, 'wake' elsewhere. The mode lasts the child's life. Say what you're waiting for and END YOUR TURN.
- WON'T ACT ON THE RESULT? relation='top': a child's report costs you a turn.
- NEVER `await_event` ON YOUR OWN CHILD's `CodingAgentIdled`, `ResponseGenerated` or `ChildThreadCompleted`: a second wake for one finish.
- `interrupted` means a crash cut its turn and nothing resumes it: follow_up_child_thread continues it. Only same-workspace children spawned with these tools report; a grandchild reports to your child, not you.
- For a pipeline where step N depends on step N-1, spawn ONE child per response and wait for the callback. Never batch sequential spawns into one response.
- SPAWN SPARINGLY. Default to doing the work yourself. Spawn only for genuinely independent subtasks that gain from running in parallel, never for what a few sequential tool calls would do, and never one thread per item in a list. At most __MAX_CONCURRENT_CHILDREN_PER_THREAD__ children running at the same time (a finished one frees its slot), maximum depth 3.

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
5. Independent tool calls go IN THE SAME RESPONSE: N file writes, or several reads and searches you already have the inputs for. Only a call that needs an earlier result waits for it.

__REPEATED_ACTION_RULE__"#;

const MEMORY_CORRECTIONS_RULE_CLASSIC: &str = "MEMORY CORRECTIONS (the `memory` tool):
- When the user says a memory is wrong (\"I don't work at Acme Corp\"), pass a broad search_query (\"Acme\") and a specific wrong_fact (\"User works at Acme Corp\"), plus an optional `correction` to replace it. A correction persists across memory rebuilds.";

const MEMORY_CORRECTIONS_RULE_TREE: &str = "MEMORY CORRECTIONS:
- Your memory is the conversation log itself, so there is nothing to edit. When the user corrects something, their words become the newest entry, and the newest ruling holds over older lines.";

fn static_prompt_body(
    has_lucidos_source: bool,
    max_tool_calls: usize,
    max_live_children: usize,
    mode: super::context_mode::ContextMode,
    memory_tree: bool,
    response_style: &str,
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
    let (memory_lookup, memory_corrections_rule) = if memory_tree {
        (MEMORY_LOOKUP_TREE, MEMORY_CORRECTIONS_RULE_TREE)
    } else {
        (MEMORY_LOOKUP_CLASSIC, MEMORY_CORRECTIONS_RULE_CLASSIC)
    };

    let body = SYSTEM_PROMPT_BASE
        .replace("__ENGINE_RESTART_RULE__", ENGINE_RESTART_RULE)
        .replace("__APPLY_VERIFY_RULE__", &apply_verify_rule)
        .replace("__LOOK_BEFORE_ASSESSING_RULE__", LOOK_BEFORE_ASSESSING_RULE)
        .replace("__MEMORY_LOOKUP__", memory_lookup)
        .replace("__MEMORY_CORRECTIONS_RULE__", memory_corrections_rule)
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
        .replace("__ENGINE_HOST_OS__", std::env::consts::OS)
        .replace("__ENGINE_HOST_ARCH__", std::env::consts::ARCH)
        .replace(
            "__AGENT_SANDBOX_PLATFORM_LIMIT__",
            AGENT_SANDBOX_PLATFORM_LIMIT,
        )
        .replace("__MAX_TOOL_CALLS__", &max_tool_calls.to_string())
        .replace(
            "__MAX_CONCURRENT_CHILDREN_PER_THREAD__",
            &max_live_children.to_string(),
        )
        .replace("__RESPONSE_STYLE_RULE__", response_style);

    format!(
        "{}{}{}",
        body,
        coding_surface_section(has_lucidos_source),
        crate::scheduler::user_tasks::TRIGGER_SYSTEM_ADDENDUM
    )
}

#[derive(Clone, Copy)]
enum MandatoryPref {
    Optional(&'static Pref<Optional>),
    Text(&'static Pref<Text>),
}

impl MandatoryPref {
    fn spec(self) -> &'static PrefSpec {
        match self {
            Self::Optional(pref) => &pref.spec,
            Self::Text(pref) => &pref.spec,
        }
    }

    async fn try_stored(
        self,
        pool: &sqlx::PgPool,
        device_id: &str,
    ) -> Result<Option<String>, sqlx::Error> {
        let per_device = self.spec().scope == PrefScope::Device;
        match (self, per_device) {
            (Self::Optional(pref), true) => pref.try_stored_for_device(pool, device_id).await,
            (Self::Optional(pref), false) => pref.try_stored(pool).await,
            (Self::Text(pref), true) => pref.try_stored_for_device(pool, device_id).await,
            (Self::Text(pref), false) => pref.try_stored(pool).await,
        }
    }
}

const MANDATORY_PREFS: &[(MandatoryPref, &str)] = &[
    (MandatoryPref::Optional(&prefs::TIMEZONE), "- TIMEZONE: Ask what timezone they are in and call preferences(action=\"set\", key=\"timezone\", value=\"…\") with an IANA name (e.g., \"America/New_York\", \"Europe/London\", \"Asia/Tokyo\")."),
    (MandatoryPref::Optional(&prefs::LANGUAGE), "- LANGUAGE: Ask what language they prefer, and mention that English is recommended for best results (the models are strongest in English). They can still write in any language; replies come back in whichever language they set here. Then call preferences(action=\"set\", key=\"language\", value=\"…\") to save it."),
    (MandatoryPref::Optional(&prefs::PUSH_NOTIFICATIONS), "- PUSH NOTIFICATIONS: Ask if they want to enable push notifications, which reach them when they are away from Lucidos: anything an agent sends them, trigger and scheduled-task results, and system alerts such as a failed backup. Never narrow them to one source such as scheduled tasks (do NOT call them \"browser\" notifications: Lucidos runs as a native desktop app too, where these are native OS alerts). When you describe how they arrive, key off the last used device in [USER DEVICE & PREFERENCES]: if its details say \"Lucidos desktop app\" they are in the native desktop app (native macOS notifications, no browser or site permission); otherwise they are in a browser/PWA (the browser will prompt for permission). If yes, call preferences(action=\"set\", key=\"push_notifications\", value=\"enabled\"). If no, call preferences(action=\"set\", key=\"push_notifications\", value=\"declined\") so you don't ask again."),
    (MandatoryPref::Text(&prefs::TECHNICAL_LITERACY), "- TECHNICAL LITERACY: Ask \"How technical should I be with you?\" as a card with exactly these three options, label then description, word for word: \"Keep it plain\" / \"Everyday words, no jargon.\"; \"Technical\" / \"Technical terms are fine.\"; \"I write software\" / \"Talk to me like a developer.\". Store the pick with preferences(action=\"set\", key=\"technical_literacy\", value=…) as \"non-technical\", \"technical\" or \"developer\". If they decline, set value=\"not-set\" so you don't ask again. Word the rest of this reply at the level they picked."),
];

fn setup_device(is_trigger: bool, device_id: Option<&str>) -> Option<&str> {
    device_id.filter(|_| !is_trigger)
}

fn setup_or_language_section(missing_instructions: &[&str], user_language: &str) -> String {
    if missing_instructions.is_empty() {
        return format!(
            "USER LANGUAGE: {}\nAlways respond in {}.",
            user_language, user_language
        );
    }
    log_legacy!(
        "[Chat] Setup required, missing preferences: {}",
        missing_instructions.join(", ")
    );
    format!(
        "SETUP REQUIRED, DO NOT PROCEED UNTIL COMPLETE:\nThe following settings are not configured. You MUST ask the user for these BEFORE doing anything else. Do NOT answer questions, create tasks, or perform any work until setup is complete.\n{}",
        missing_instructions.join("\n")
    )
}

#[easy_ext::ext(SystemPromptExt)]
pub(crate) impl LucidosEngine {
    async fn build_chat_system_prompt(
        &self,
        user_timezone: &str,
        user_language: &str,
        device_id: Option<&str>,
        trigger: &Option<TriggerContext>,
        max_tool_calls: usize,
        capabilities: &crate::engine::tools::TurnCapabilities,
    ) -> (String, Vec<&'static str>) {
        let timezone_section = super::turn_clock::timezone_section(user_timezone);

        let mut missing_instructions = Vec::new();
        let mut missing_pref_keys = Vec::new();
        let prefs_to_check = match setup_device(trigger.is_some(), device_id) {
            Some(did) => MANDATORY_PREFS.iter().map(|pref| (did, pref)).collect(),
            None => Vec::new(),
        };
        for (did, (pref, instruction)) in prefs_to_check {
            let key = pref.spec().key;
            let value = match pref.try_stored(&self.pool, did).await {
                Ok(v) => v,
                Err(e) => {
                    log_legacy!(
                        "[Chat] preference read failed for '{}': {}. Treating as configured so the turn is not refused",
                        key,
                        e
                    );
                    continue;
                }
            };
            if value.is_none() {
                missing_instructions.push(*instruction);
                missing_pref_keys.push(key);
            }
        }

        let language_section = setup_or_language_section(&missing_instructions, user_language);

        let system_prompt = workspace_identity_section(
            &self.workspace_name(),
            &self.workspace_path,
            &timezone_section,
            &language_section,
        );

        let has_lucidos_source = crate::paths::has_lucidos_source();

        let context_mode = super::context_mode::ContextMode::from_capabilities(&capabilities.gates);

        let response_style = crate::core::response_style::resolve(&self.pool).await;

        let max_live_children = self
            .thread_queue
            .policy()
            .await
            .max_concurrent_children_per_thread;

        let system_prompt = format!(
            "{}{}",
            system_prompt,
            static_prompt_body(
                has_lucidos_source,
                max_tool_calls,
                max_live_children,
                context_mode,
                capabilities.gates.memory_tree,
                &response_style
            )
        );

        let system_prompt = if has_lucidos_source {
            format!(
                "{}{}",
                system_prompt,
                super::turn_tail::ENGINE_BUILD_POINTER
            )
        } else {
            system_prompt
        };

        let system_prompt = format!("{}{}", system_prompt, super::turn_tail::CLIENT_URL_POINTER);
        let system_prompt = format!(
            "{}{}",
            system_prompt,
            super::child_status::CHILD_STATUS_POINTER
        );

        // INVARIANT: No auto-detected browser-login domain list here, deliberately:
        // `docs/adr/0067-browser-login-domains-never-reach-the-prompt.md`.

        // INVARIANT: A read that FAILED is not a workspace with no apps, and the prompt
        // renders the two identically. So the agent tells the user they have
        // none, and nothing anywhere says the scan broke. Same treatment as the
        // preference read above: degrade the turn, but name what happened.
        let automatic_widgets = crate::core::prefs::AUTOMATIC_WIDGETS.read(&self.pool).await;
        let apps_section = match super::workspace_prompt_footprint::apps_section(
            &self.app_manager,
            automatic_widgets,
        ) {
            Ok(section) => section,
            Err(e) => {
                log_legacy!(
                    "[Chat] app list read failed ({}); this turn's prompt names no apps",
                    e
                );
                String::new()
            }
        };

        let intents_section =
            super::workspace_prompt_footprint::build_intents_section(&capabilities.intents);

        let (knowhow_summaries, app_knowhow_summaries) = self.workspace_knowhow_summaries();
        let knowhow_section = super::workspace_prompt_footprint::build_knowhow_section(
            &knowhow_summaries,
            &app_knowhow_summaries,
        );

        let system_knowhow_section = build_system_knowhow_section(&self.system_knowhow_summaries());

        let system_prompt = format!(
            "{}{}{}{}{}",
            system_prompt, apps_section, intents_section, knowhow_section, system_knowhow_section
        );

        let system_prompt = {
            let mut section = format!("{}\n\n## Images\n\n\
                Every image in the conversation has an 'img-<hex>' handle, printed beside it. \
                The conversation history notes earlier images with their handle and thread:N index \
                (e.g. \"[attached image (thread:2, img-…)]\"). Images in the message content are labeled \
                \"from earlier in the conversation\" or \"attached to current message\", and the label \
                on current images carries their handles. \
                To name an image in a tool call, copy its handle. Do not count to work out thread:N. \
                Older images age out of your vision after a few messages — the history then shows only a \
                text note like \"[attached image (thread:2, img-…) — image not included]\" plus a description. \
                When the user refers to an image you can no longer see, call the view_image tool with its \
                handle to load it back into your vision, then answer from what \
                you see — do NOT claim you have no image or ask the user to re-send it. \
                You can save any conversation image to an artifact file with the save_thread_image tool \
                (image: its handle, path: under artifacts/, e.g. 'artifacts/photos/reaction.jpg').", system_prompt);
            if capabilities.gates.image_provider {
                section.push_str(
                    " You can also generate or edit images with the generate_image tool. \
                    To edit an existing image, pass its handle in input_images, \
                    or use an artifact path like 'artifacts/photo.png'. \
                    When the user says \"edit the second image\", thread:2 also works.",
                );
            }
            section
        };

        let system_prompt = format!(
            "{}{}",
            system_prompt,
            super::context_mode::system_prompt_section(context_mode, capabilities.schedule)
        );

        (system_prompt, missing_pref_keys)
    }
}

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

pub(crate) fn chat_tools(
    gates: &crate::llm::ToolCapabilities,
) -> Vec<crate::llm::provider::ToolDefinition> {
    let mut tools = crate::llm::tools::get_default_tools(gates);
    tools.push(crate::llm::tools::get_notification_tool());
    tools.extend(crate::capability_manifest::llm_tools_for(gates));
    tools.extend(crate::llm::tools::chat_tail_tools(gates));
    tools
}

pub(crate) fn flat_chat_tools(
    gates: &crate::llm::ToolCapabilities,
) -> Vec<crate::llm::provider::ToolDefinition> {
    let grouped: std::collections::HashSet<String> =
        crate::capability_manifest::llm_tools_for(gates)
            .into_iter()
            .map(|t| t.name)
            .collect();
    chat_tools(gates)
        .into_iter()
        .filter(|t| !grouped.contains(&t.name))
        .collect()
}

pub(crate) fn wire_chars(tool: &crate::llm::provider::ToolDefinition) -> usize {
    serde_json::to_string(tool)
        .expect("a tool schema serializes")
        .chars()
        .count()
}

pub(crate) fn system_prompt_footprint(
    gates: &crate::llm::ToolCapabilities,
    system_knowhow: &[crate::core::KnowhowSummary],
    built_in_widgets: &[crate::core::App],
) -> Vec<(&'static str, usize)> {
    use super::context_mode::{ContextMode, SweepSchedule};
    use super::turn_tail::{
        build_statuses, client_url_block, engine_build_block, CLIENT_URL_POINTER,
        ENGINE_BUILD_POINTER,
    };

    let widest_style = crate::core::response_style::widest_shipped_section();
    let child_cap = crate::engine::thread_queue::DEFAULT_MAX_CONCURRENT_CHILDREN_PER_THREAD;
    let body = [(true, false), (false, false), (true, true), (false, true)]
        .into_iter()
        .map(|(source, memory_tree)| {
            static_prompt_body(
                source,
                500,
                child_cap,
                ContextMode::Off,
                memory_tree,
                &widest_style,
            )
            .chars()
            .count()
        })
        .max()
        .expect("four bodies");

    let flat: usize = flat_chat_tools(gates).iter().map(wire_chars).sum();
    let grouped: usize = crate::capability_manifest::llm_tools_for(gates)
        .iter()
        .map(wire_chars)
        .sum();
    let knowhow = build_system_knowhow_section(system_knowhow).chars().count();

    let engine_build = ENGINE_BUILD_POINTER.chars().count()
        + build_statuses()
            .into_iter()
            .map(|status| engine_build_block(&status).chars().count())
            .max()
            .expect("build states");
    let client_url = CLIENT_URL_POINTER.chars().count() + client_url_block("").chars().count();
    let child_status = super::child_status::CHILD_STATUS_POINTER.chars().count()
        + super::child_status::render_child_status(Err(()))
            .chars()
            .count();

    let context_mode =
        super::context_mode::system_prompt_section(ContextMode::On, SweepSchedule::default())
            .chars()
            .count();

    let widgets_rules = [true, false]
        .into_iter()
        .map(|automatic| {
            crate::engine::widget_guidance::widgets_rules(
                crate::engine::widget_guidance::WidgetAudience::Chat { automatic },
            )
            .chars()
            .count()
        })
        .max()
        .expect("two forms");
    let built_in_widgets = crate::engine::widget_guidance::widgets_list(built_in_widgets)
        .chars()
        .count();

    vec![
        ("static prompt body + rule consts", body),
        ("## Widgets rules, wider form", widgets_rules),
        ("built-in widgets list", built_in_widgets),
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
        ("CHILD THREADS: pointer + failed-read block", child_status),
    ]
}

#[cfg(test)]
mod tests {
    use super::super::super::process_helpers::{
        APPLY_VERIFY_DEV_ADDENDUM, APPLY_VERIFY_RULE, LOOK_BEFORE_ASSESSING_RULE,
        MEMORY_LOOKUP_CLASSIC, MEMORY_LOOKUP_TREE,
    };
    use super::super::context_mode::ContextMode;
    use super::{
        coding_surface_section, setup_device, setup_or_language_section, static_prompt_body,
        system_prompt_footprint, wire_chars, workspace_identity_section, ASK_USER_QUESTION_RULE,
        MANDATORY_PREFS, MEMORY_CORRECTIONS_RULE_CLASSIC, MEMORY_CORRECTIONS_RULE_TREE,
        NAMES_NOT_IDS_RULE, NO_IMPERSONATION_RULE, SETUP_INTERVIEW_RULE, SYSTEM_PROMPT_BASE,
        TRIGGER_VS_EVENT_WAIT_RULE, WORKSPACE_ASSETS_KNOWHOW_RULE,
    };
    use crate::core::prefs::{self, PrefScope};
    use crate::engine::thread_queue::DEFAULT_MAX_CONCURRENT_CHILDREN_PER_THREAD as CHILD_CAP;
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
    ///   return the three states stop rewriting the whole 21,668-token system
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
    ///
    /// Raised by 362 to a measured 115,941 to stop the resident text claiming
    /// that acting in another workspace is impossible. It is not:
    /// `lucidos spawn-thread --to` does it. A chat thread believed the old
    /// wording and told the user so, then hunted for workarounds. Deleting the
    /// false clause was not enough on its own. An agent just told "not that"
    /// has to be told "this instead" in the same breath, and knowhow cannot
    /// carry it: it decides the thing is impossible before it would load a doc.
    ///
    /// Raised by 217 to a measured 116,158 for the image line in FILE
    /// REFERENCES. A link had been the agent's whole vocabulary for showing a
    /// file. The incident is in
    /// `the_file_references_section_says_an_image_is_drawn_not_linked`.
    /// Knowhow cannot carry this one: writing the message is the moment the
    /// choice is made, and nothing prompts a lookup first.
    ///
    /// The line itself is 285 chars, so the ratchet moved less than the line
    /// costs. The previous ceiling carried 68 chars of slack, and this
    /// absorbed it. Every row below is a measured total, never a rounded one.
    ///
    /// Raised by 140 to a measured 116,298 for the *app fragment* clause on
    /// the app-link line. An app link named the app and nothing inside it. So
    /// an agent publishing a per-item artifact into a shared app could only
    /// link the app and hope. Knowhow cannot carry it, for the reason the
    /// image line could not: writing the link is the moment the choice is
    /// made, and nothing prompts a lookup first.
    ///
    /// Raised by 254 to a measured 116,552 for `fragment` on the two schemas
    /// that build a navigation: `navigate_ui` and `send_notification`'s `tap`.
    /// A notification tap could open an app and name no place inside it. So a
    /// notification about one item landed on whatever the app shows by
    /// default. Knowhow cannot carry it, for the reason the app-link clause
    /// could not: the tap is written in the same breath as the notification.
    ///
    /// Raised by 21 to a measured 116,573 for `send_email`'s attachment
    /// wording. Attachments used to be validated for traversal only, so a path
    /// naming the `data/` root mailed the workspace `.env` to any address the
    /// model chose. The validator now takes a typed subdirectory, and a schema
    /// still saying "relative to data/" would send the model at a path that is
    /// refused. Knowhow cannot carry it: the schema is what the model reads
    /// when it writes the argument.
    ///
    /// Raised by 58 to a measured 116,631 for `navigate_ui`'s
    /// `system-overview`. Settings > System became a submenu, so `system` now
    /// opens a list of rows. The versions, the update control and the restart
    /// moved to a page of their own. Without a value naming that page, the
    /// model cannot ask for them at all. Knowhow cannot carry it: the enum is
    /// what the model picks from when it writes the argument.
    ///
    /// Raised by 132 to a measured 116,763 for the `secret` credential type.
    /// The agent's only always-loaded list of the types omitted it, so it told
    /// a user the type did not exist. The list is now rendered from `AuthType`,
    /// and one line says what a `secret` is for. Knowhow cannot carry it: the
    /// enum is what the model picks from when it writes the argument.
    /// Raised by 446 to a measured 117,209 for the two *standing apply*
    /// actions on the `changes` tool. ADR 0168 puts the standing apply on both
    /// change surfaces. Philosophy rule 2 then requires it through the prompt
    /// too. Knowhow cannot carry it: the action enum is what the model picks
    /// from when it writes the argument.
    ///
    /// Raised by 161 to a measured 117,370 for `cancel_standing_apply`, the off
    /// for those two arms. The prompt could arm and never disarm, which is
    /// philosophy rule 2 half-kept. One action covers both scopes, so the cost
    /// is a single entry. Knowhow cannot carry it: the action enum is what the
    /// model picks from when it writes the argument.
    ///
    /// Raised by 160 to a measured 117,530 for `request_credential`'s
    /// `base_urls`. A credential scope has been a SET since ADR 0161, and the
    /// singular argument was the last layer that could not say so. So a token
    /// serving a provider's API host and its git host was stored twice, under
    /// two service names, with two places to rotate it. Knowhow cannot carry
    /// it: the argument is what the model writes.
    /// Raised by 382 to a measured 117,912 for the *response style*, billed at
    /// its widest shipped section: the wrapper, the instruction, and the floor
    /// that keeps a warning in an answer however short the style asks for.
    ///
    /// It buys back more than it spends, on the turns that matter. Two users on
    /// the packaged build were switching models by hand to escape the default's
    /// length, and one was drifting off the default model entirely. A style that
    /// trims a reply pays for these characters in the first answer it shortens.
    ///
    /// The meter bills the SHIPPED text only. A style the user wrote is
    /// workspace content, the same class as `user_profile.md`, and this ratchet
    /// measures the engine-authored surface. Standard, the default, is billed
    /// here and injects nothing at all.
    ///
    /// Raised by 65 to a measured 117,977 for a todo list the agent can read.
    /// The `todo_write` schema itself grew 367, from 1,063 to 1,430. It buys
    /// the `action` enum and a description saying the call answers with the
    /// resulting list.
    ///
    /// The ceiling moves by 65 rather than 367, because it was carrying 302
    /// characters of slack. This change spends them. That is the silent spend
    /// the rule above warns about, recorded here rather than left unsaid.
    ///
    /// What it buys pays for itself on a long thread. The engine settles every
    /// unfinished item at each response terminator, and nothing carried that
    /// rewrite back. Items the agent had landed went on reading `waiting` for
    /// hours. A count cannot correct that, and the list can.
    ///
    /// Raised by 42 to a measured 118,019 for Claude Fable 5.1 in the
    /// coding-agent pickers. The `run_coding_agent` `model` enum is the union
    /// of both backends' picker values, so every new row is billed on every
    /// chat request. Two rows here, the pinned id and its 1M variant.
    ///
    /// Not offering the model is the only way to avoid the cost:
    /// `validate_coding_agent_model` refuses an id no picker carries, so an
    /// enum the user's picker has outgrown just makes the tool call fail.
    ///
    /// Raised by 66 to a measured 118,085 for the `changes` apply summary,
    /// which now says the call is refused while its thread is unsettled. The
    /// refusal was the bug (ADR 0233). An agent that learns it only by trying
    /// has already asked to merge a branch somebody is still writing to.
    ///
    /// Raised by 599 to a measured 118,684 for the DANGLING ITEM paragraph in
    /// `ASK_USER_QUESTION_RULE`. A decision phrased as a plain statement carries
    /// no question mark, so the syntactic QUESTION-SHAPE test above it never
    /// fires. The paragraph routes an outstanding, parked, or newly unblocked
    /// item to a card in the same turn.
    ///
    /// Raised by 284 to a measured 118,968 for the menu-as-addendum clause
    /// closing ANSWER FIRST in the same rule. That paragraph banned a what-next
    /// menu without ever saying when to raise one. A model reading it after a
    /// full answer suppressed the card it owed. The clause pays for itself: the
    /// gate that otherwise catches this costs a whole extra round.
    ///
    /// Raised by 40 to a measured 119,008 for Claude Opus 5.5 in the
    /// coding-agent pickers, on the same terms as the Fable 5.1 clause above.
    /// Two rows again, the pinned id and its 1M variant.
    ///
    /// Raised by 110 to a measured 119,118 for CRITICAL RULE 5, which widened
    /// from N file writes to every independent call. 85% of rounds carried a
    /// single tool call, and each removed round saves a full context resend.
    ///
    /// Raised to a measured 119,865 for one model over many providers.
    /// `triggers` gains the `provider` pin, a null-union over the seven
    /// provider names. `manage_models` gains `routes`, an LLM `update` action
    /// with `preferred_provider`, and a window that `null` clears.
    ///
    /// Knowhow cannot carry either: the model writes the argument from the
    /// enum and the route shape. Editing routes from the prompt is the parity
    /// the plan settled (philosophy rule 2).
    ///
    /// Raised by 99 to a measured 119,964 for `navigate_ui`'s `device`. A
    /// navigate reaches exactly one device, and "open it on my Mac" from the
    /// phone is decided while writing the call. The old ceiling carried 2
    /// chars of slack, which this absorbed.
    ///
    /// Raised by 358 to a measured 120,322 for *technical literacy*, the
    /// response style's second part. The meter bills its widest level on top
    /// of the widest style. A workspace with no level set pays nothing.
    ///
    /// Raised by 408 to a measured 120,730 for the same design's second pass:
    /// the lower levels' relevance and steering rules, the reworded Minimal and
    /// the Learning style. Both are still billed at their widest shipped case.
    ///
    /// Raised by 289 to a measured 121,019 for `threads` 'detach_child'
    /// (ADR 0278), and its routing phrase in the orchestrating knowhow. A
    /// parent with no way to stop waiting for a child can only stop and
    /// respawn it, which loses the child's work. It rides the grouped tool
    /// rather than a standalone one, which would cost several times this.
    ///
    /// Raised by 120 to a measured 121,139 for one clause in FILE REFERENCES:
    /// saving a picture shows the user nothing, and a picture offered for a
    /// choice goes in an option's description. A coding agent saved a mockup,
    /// said it had drawn the options, and showed nothing. The user asked for
    /// the Lucidos agent to carry the same rule.
    ///
    /// Raised by 298 to a measured 121,437 for the required `description` on
    /// `run_bash_background` and `run_python_background`. The engine-armed wait
    /// names the work by it. Before, the row said only "background work", and
    /// the user could not tell what the thread was waiting on.
    ///
    /// Raised by 131 to a measured 121,568 for the visual-choice clause in
    /// FILE REFERENCES: real pictures, one per option, never ASCII art. A
    /// coding agent drew colour options in ASCII, then repeated one sheet of
    /// all four on every option.
    ///
    /// Raised by 160 to a measured 121,728 for themes: the `themes` knowhow's
    /// routing line and the `theme` preference. The routing line is how the
    /// agent learns a user can ask for a theme. So a theme is reachable from
    /// the prompt, not only from Settings (docs/plans/2026-09-26-looks.md).
    ///
    /// Raised by 416 to a measured 122,144 for `threads` 'archive' and its rule
    /// in PARALLEL WORK (ADR 0310). Nothing archives a thread on its own, so an
    /// orchestrator is told when to close the children it applied.
    ///
    /// Raised by 199 to a measured 122,343 for workspace fonts: the
    /// `workspace-fonts` knowhow's routing line, and `ws-<slug>` in the
    /// `font-family` preference. The routing line is how the agent learns it
    /// can install a font (docs/plans/2026-09-27-workspace-fonts.md).
    ///
    /// Lowered by 392 to a measured 121,951: `run_coding_agent`'s `model` is a
    /// plain string now, checked at the spawn (ADR 0325). Claude Code's picker
    /// is discovered per workspace, so a picker row no longer costs a byte here
    /// and the per-model raises above stop recurring.
    ///
    /// Raised by 57 to a measured 122,008: `ask_user_question` says its
    /// question renders markdown. A card of findings arrived as one run-on
    /// paragraph without it.
    ///
    /// Lowered by 260 to a measured 121,748: the PARALLEL WORK rule on when to
    /// archive is gone (ADR 0330). No prompt says when to archive.
    ///
    /// Raised by 409 to a measured 122,157 for `threads` 'drafts' and
    /// 'held_messages', and the `has_draft` / `has_diff` filters. The agent
    /// could not say which thread held a user's unsent draft, nor link it
    /// (docs/plans/2026-10-02-threads-tool-which-thread-holds-what.md).
    ///
    /// Raised by 131 to a measured 122,288: `run_coding_agent`'s `prompt` now
    /// says review, merge and hardening behavior are defaults set by the
    /// folder, stated only to ask for something else. An orchestrator was
    /// restating them in a child's brief as a prohibition. A stray "do not
    /// harden" made a session skip a step its own kind required.
    ///
    /// Raised by 422 to a measured 122,710 for `threads` 'triage' and
    /// 'apply_triage' (ADR 0349). A tester with a pile of stale threads had no
    /// safe way to clear them. The how lives in the triage result, not here.
    ///
    /// Raised by 478 to a measured 123,188 for the builtin provider proxies.
    /// `proxy_request` names them and the /v1 base quirk, `request_credential`
    /// says it refuses a service one already serves, and the API-data line
    /// points at them. An agent opened a key form for OpenAI while the engine
    /// already held a working OpenAI proxy
    /// (docs/plans/2026-10-02-agent-sees-builtin-provider-proxies.md).
    ///
    /// Raised by 118 to a measured 123,306 for the tool order in BROWSER
    /// TOOLS: a configured proxy or an MCP tool first, the browser last and
    /// announced. No line gave an order, so nothing kept the agent off a site
    /// a proxy already served. The choice is made at the call, where no
    /// knowhow loads.
    ///
    /// Raised by 365 to a measured 123,671: PARALLEL WORK now says a direct
    /// child reports back on its own. It names the finishing events never to
    /// wait on, and what `interrupted` asks of the parent. An orchestrator
    /// subscribed to its own child's idle, and a workspace knowhow was
    /// carrying a rule the engine should state
    /// (docs/plans/2026-10-03-crash-cut-child-reports-truthfully.md).
    ///
    /// Raised by 39 to a measured 123,710 for the Settings link in FILE
    /// REFERENCES: `[Backup](settings:backup)` opens one Settings page. The
    /// agent sends users to Settings pages often and could only name the route
    /// (docs/plans/2026-10-05-backup-settings-links-and-sleep.md).
    ///
    /// Raised by 375 to a measured 124,085 for the card's `message`: the
    /// `ask_user_question` field and the paragraph after ANSWER FIRST that
    /// routes an answer into it. On Opus 5.5 prose before a tool call may
    /// arrive only as a summary, and steps with links never reached the user
    /// (docs/plans/2026-10-06-chat-card-message-field.md).
    ///
    /// Raised by 120 to a measured 124,205 for the picture rule in FILE
    /// REFERENCES. A picture shows only in text that ends the turn or on the
    /// card. "Draw it in your reply" sent mockups into text a tool call
    /// followed, whose summary drops them
    /// (docs/plans/2026-10-06-chat-pictures-go-in-the-card-message.md).
    ///
    /// Raised by 88 to a measured 124,293 for the child cap (ADR 0380). PARALLEL
    /// WORK says the cap is on children running at the same time, and the
    /// `thread_queue` schema gains `max_concurrent_children_per_thread`. An
    /// agent read a lifetime cap and moved its work out of the family
    /// (docs/plans/2026-10-06-child-cap-counts-live-children.md).
    ///
    /// Raised by 91 to a measured 124,384 for the `#L` bullet in FILE
    /// REFERENCES. The agent put a cited range only in a link's label, so a
    /// click opened the file at the top
    /// (docs/plans/2026-10-08-transcript-file-links-open-the-preview-modal.md).
    ///
    /// Raised by 15 to a measured 124,399 for the `threads` filter
    /// `change_state`, whose three-value enum replaced the `has_diff` boolean
    /// (ADR 0400). Its description was cut to the minimum first.
    ///
    /// Raised by 1,116 to a measured 125,515 for widgets (ADR 0402): the
    /// grouped `widgets` tool, `create_app`'s `kind` arg, the routing row, and
    /// `building-an-app`'s description naming widgets. Every widget action
    /// needs an agent path (plan invariant 9), and each was cut first.
    ///
    /// Raised by 479 to a measured 125,994 for the `request_read` tool and its
    /// trigger clause (ADR 0409). Only the thread knows whether its reply is
    /// worth reading, and the description says when not to ask. Cut first.
    ///
    /// Raised by 858 to a measured 126,852, with no new text: the `## Widgets`
    /// rules were sent all along and metered nowhere. They go out with or
    /// without a widget, so they are system prompt footprint (ADR 0413).
    ///
    /// Raised by 161 to a measured 127,013 for `create_app`'s `icon_svg` arg
    /// (ADR 0414). The agent draws an icon for every app and widget, and the
    /// arg is how. Its description was cut to one sentence first.
    ///
    /// Raised by 1,159 to a measured 128,172 for widget params and embeds (ADR
    /// 0415). It pays for the `widgets` tool's `params` and `label` args and
    /// the embed in the question option's `description`. It also pays for one
    /// FILE REFERENCES sentence, the embed rule in `## Widgets`, and the
    /// built-in widgets' lines, which every workspace is sent. An agent must
    /// name an instance and write an embed, and each text was cut first.
    ///
    /// Raised by 151 to a measured 128,323 for the read decision (ADR 0417):
    /// `request_read`'s required `read` argument and the description saying
    /// when to decide and when to leave it to the engine.
    ///
    /// Raised by 117 to a measured 128,440 for `capture_app`'s
    /// `save_as_artifact`, which lets the agent take a plugin's screenshots
    /// itself. The tool's own description was cut to pay for most of it.
    ///
    /// Raised by 207 to a measured 128,647 for the widget look in `## Widgets`:
    /// one shape instead of a table, the three shape classes, no title and no
    /// border inside the card. The full rules are in `building-an-app`; this
    /// anchor keeps the core of them in view while the agent writes the HTML.
    ///
    /// Raised by 279 to a measured 128,926 for the built-in video player's
    /// line and the embed rule naming a video clip. Without them the agent
    /// offers a choice between films as labels, with nothing to watch.
    ///
    /// Raised by 193 to a measured 129,119 for `unload_knowhow` (ADR 0425), so
    /// the agent can drop a loaded knowhow doc it is done with. A loaded doc
    /// costs thousands of tokens on every turn it stays. The schema was cut
    /// to its id argument, and the lifecycle text lives in the block header.
    ///
    /// Raised by 101 to a measured 129,220 for `capture_app`'s `device`. The
    /// agent takes a plugin's screenshots on the phone while the turn's last
    /// used device is the laptop. The description is `navigate_ui`'s own.
    ///
    /// Raised by 34 to a measured 129,254 for `create_app`'s `name` saying no
    /// emoji. An emoji in the name draws a second icon beside the app's own.
    ///
    /// Raised by 239 to a measured 129,493 for the `mcp` tool's `enable` and
    /// `disable` actions (ADR 0430). A running server's tools now reach only
    /// the threads that enable it, which takes its schemas off every other
    /// request.
    ///
    /// Raised by 154 to a measured 129,647 for the `triggers` tool's
    /// `mcp_servers` (ADR 0430). A trigger's fires may call only the MCP
    /// servers it lists, so the agent that writes a trigger has to list them.
    ///
    /// Raised by 674 for filed reports: FAN-OUT and the spawn tools say a
    /// clean report wakes the parent only when the child says it must act. A
    /// parent that believed every report re-opens it would wait for a wake
    /// that never comes.
    ///
    /// Raised by 847 to a measured 131,168 for the board (ADR 0432): the
    /// `widgets` tool's board and arrange actions, `create_app`'s `sizes`, the
    /// `home-board` navigate target, each listed widget's sizes, and one
    /// `## Widgets` sentence. It says to arrange Home only when the user asks,
    /// and never to size a new pin. Every arrangement the UI makes needs an
    /// agent path (plan invariant), and each text was cut first.
    ///
    /// Raised by 30 to a measured 131,198 for ADR 0472: the `TRIGGERS` rule
    /// now asks every trigger run for its own read decision, which keeps
    /// routine runs out of Review. Its wording was cut first, and
    /// `request_read`'s description dropped the duplicate clause.
    ///
    /// Raised by 975 to a measured 132,173 for the trigger rules. The 975-char
    /// `TRIGGER_SYSTEM_ADDENDUM` joined the static body of every turn, where
    /// a trigger fire's system block now equals a chat turn's. Fires and chat
    /// turns then share one cache entry for the tools and the system block.
    /// Before, only a trigger's own system block carried the text.
    ///
    /// Raised by 132 to a measured 132,305 for ADR 0496: the `widgets` tool
    /// gained `add_to_dashboard` and `remove_from_dashboard`, since each
    /// dashboard now holds its own tiles and every UI change needs an agent
    /// path. Its summaries and schema text were cut first.
    ///
    /// Raised by 177 to a measured 132,482 for ADR 0505: the routing line of
    /// `system-knowhow/coding-agent-flows`, which wires a repository's handoff
    /// checkpoints. Its description was cut to one clause first, and the two
    /// default flows sit in its folder, where they cost no line.
    ///
    /// Raised by 30 to a measured 132,512 for ADR 0508: the eighth provider
    /// name, `claude-gateway`, in the `triggers` and `manage_models` enums.
    /// The tools validate against those enums, so leaving it out would make
    /// them lie.
    ///
    /// Raised by 207 to a measured 132,719 for ADR 0525: the routing line of
    /// `system-knowhow/model-price-check`, the recipe the Keep prices current
    /// trigger routes on. Its description was cut to one clause first.
    ///
    /// Raised by 4 to a measured 132,723 for `request_read`: a turn that
    /// asks the user a question skips the read decision.
    ///
    /// Lowered by 413 to a measured 132,310: the retired `execute_intent` schema left
    /// the tools array with its sub-loop.
    ///
    /// Raised by 201 to a measured 132,511 for the `coding_agent` and `folder`
    /// args of `navigate_ui`'s `new-chat` target. Both descriptions were cut
    /// to one clause first.
    ///
    /// Raised by 279 to a measured 132,790 for the routing line of
    /// `system-knowhow/stored-secrets`. The agent role cannot see secret
    /// columns, so without it the agent concluded secrets were not stored.
    ///
    /// Raised by 135 to a measured 132,925 for the `quiet` option field of
    /// `ask_user_question`, the one way to offer a pick that ends the turn.
    ///
    /// Raised by 197 to a measured 133,122 for `threads` 'cache_keep_alive'
    /// (ADR 0554), so a user can turn a thread's keep-alive off in words
    /// (prompt-first). Its summary is one clause, and `state` has no
    /// description beyond its enum.
    ///
    /// Raised by 566 to a measured 133,688 for the `expect_wake` tool (ADR
    /// 0603), which the cache keep-alive plans its pings from. Its description
    /// was cut to four short sentences first.
    ///
    /// Lowered by 4 to a measured 133,684: the backups routing line names
    /// snapshots in fewer characters.
    ///
    /// Raised by 70 to a measured 133,754 on a `macos` (`aarch64`) build for
    /// the ENGINE HOST line, net of slack freed since the last measure.
    /// Without it, a cloud agent took the user's Mac browser for its own
    /// host. A `linux` (`x86_64`) build measures 1 less.
    ///
    /// Raised by 765 to a measured 134,519 for the team workspace (ADR 0623):
    /// the trigger tool's `continue_key` and `actor` fields, and the
    /// `team-workspace` routing line. All three descriptions were cut first.
    ///
    /// Raised by 243 to a measured 134,762 for the `threads` tool's
    /// `context_ceiling` action (ADR 0629), the prompt-first path to the
    /// Settings ceiling. Its summary was cut to one clause first.
    ///
    /// Raised by 38 to a measured 134,800 for the `xl` widget size (ADR 0633):
    /// one more value in each size enum, and the widget rules naming it. That
    /// rule was cut to one clause first.
    ///
    /// Raised by 17 to a measured 134,817 for `providers`, a `settings_view`
    /// of its own, and `fragment` naming a settings row. A link to a provider's
    /// block is written while the agent writes the call. The `fragment`
    /// description was cut to the original's length first.
    ///
    /// Raised by 30 to a measured 134,847 for ADR 0636: the ninth provider
    /// name, `chatgpt-plan`, in the `triggers` and `manage_models` enums,
    /// which the tools validate against.
    ///
    /// Raised by 82 to a measured 134,929 for the widget fit check (ADR 0643):
    /// the `fit` action on the `widgets` tool, and one widget rule naming its
    /// note and the ellipsis ban. The stale title-bar clause was cut, and both
    /// texts trimmed, first.
    ///
    /// Raised by 8 for `people`, a `settings_view` of its own. People left
    /// the Devices page, where `devices` used to reach it.
    ///
    /// Raised by 618 to a measured 135,555 on a `macos` build for the agent
    /// sandbox (ADR 0645): the AGENT SANDBOX line and the `agent-sandbox`
    /// routing entry. Without them a permission error reads as a bug to work
    /// around. Both were cut first, and the Linux limit is the shorter one.
    ///
    /// Raised by 490 to a measured 136,045 for `threads` 'submit_draft' (ADR
    /// 0651): a summary and three properties. Without it a chat agent the
    /// owner told to send a saved draft had no way to. Both texts were cut
    /// first.
    ///
    /// Raised by 157 to a measured 136,202 for thread liveness (ADR 0679): the
    /// look rule's thread-verdict bullet and the `events` query naming its
    /// header. A working thread was called stalled from a reply. Both texts,
    /// and the `byte_limit` description, were cut first.
    ///
    /// Raised by 10 for the deferred-build ENGINE BUILD state, which landed
    /// with no raise and left this test red at 136,212.
    ///
    /// Raised by 452 to a measured 136,664 for the child-status block (ADR
    /// 0679, amended): its pointer and failed-read notice (281), and the look
    /// rule binding every reply (171). A child's stale report was repeated as
    /// its state now. All three texts were cut first.
    ///
    /// The trigger read rule and `request_read` description now say the
    /// judge reviews a trigger run's yes. The two texts fit in the existing
    /// ceiling.
    ///
    /// Raised by 68 to a measured 136,732 for `manage_models` `reason` (ADR
    /// 0750): the owner's card quotes it. Without it the card for an agent's
    /// ladder edit says nothing about why. The schema's other texts were
    /// already terse, and the context-window warning stays.
    ///
    /// Raised by 133 to a measured 136,865 for ahead-of-run trigger grants:
    /// `program_grants` and `folder_grants` let an agent ask for a trigger's
    /// grants when it creates the trigger, before any run fails.
    ///
    /// Raised by 503 to a measured 137,368 for event hooks (ADR 0746): the
    /// `event-hooks` and `building-a-software-factory` routing lines, and the
    /// workspace assets route to the first. A hook fails closed, so one an
    /// agent improvises can stop a workspace's progress. Both descriptions were
    /// cut to about 180 characters first.
    const ALWAYS_LOADED_BUDGET_CHARS: usize = 137_368;

    const BILLED: ToolCapabilities = ToolCapabilities {
        email_account: true,
        image_provider: false,
        judgment_provider: false,
        context_mode: false,
        memory_tree: false,
    };

    fn flat_chat_tools() -> Vec<crate::llm::provider::ToolDefinition> {
        super::flat_chat_tools(&BILLED)
    }

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

    const PER_TOOL_SCHEMA_CEILING_CHARS: usize = 1_500;

    const PER_TOOL_CEILING_EXCEPTIONS: &[(&str, usize, &str)] = &[
        (
            "create_app",
            1_583,
            "it makes both kinds, so it carries the app-only scaffold guidance \
             and the widget-only args together. Past the shared ceiling by \
             `sizes` (ADR 0432): a widget's tile sizes are chosen while \
             writing the call that creates it, and its enum is the frozen size \
             contract plugins build on",
        ),
        (
            "widgets",
            1_715,
            "a dashboard's arrange action (ADR 0432) takes a list of tiles, \
             which no scalar arg can express, so it carries its own schema with \
             the frozen size and dashboard enums. Arranging is prompt-first: \
             every change a dashboard's edit mode makes has this one agent path. \
             Its add and remove actions are the agent's path to the two \
             dashboards' own tiles (ADR 0496)",
        ),
        (
            "await_event",
            2_845,
            "four tests pin thirteen distinct phrases on it (forward-only plus \
             the arming race, spent-and-resubscribe, the recent-subscription cap, the \
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
            4_166,
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
             and the field-path rule land here rather than in the knowhow. \
             Raised from 2,980 by the trigger `provider` pin: a null-union over \
             the provider names, declared once on create like `model`, and \
             from 3,401 by the eighth, `claude-gateway` (ADR 0508). \
             Raised from 3,907 by the ninth, `chatgpt-plan` (ADR 0636). \
             Raised from 3,250 by `mcp_servers` (ADR 0430): a null-union over a \
             string array, since a trigger's fires may call only the MCP servers \
             it lists. Raised from 3,418 by ADR 0623: `continue_key` is a \
             null-union over an array of strings or string arrays, and `actor` \
             a null-union string, each with one trimmed sentence. Raised from 3,922 by ahead-of-run grants: `program_grants` and \
             `folder_grants` on create and update, two string arrays with one \
             short sentence each, so the owner sees every grant on one card \
             before the first run",
        ),
        (
            "manage_models",
            2_058,
            "six actions, the nine-value provider enum, and `routes`: an array \
             of objects whose item shape (provider, wire id, window) is what \
             the model writes a multi-backend model from. The context-window \
             warning stays, because an omitted window silently budgets a large \
             non-Claude model at 200k. Its type is a null-union, so update can \
             clear a window. Raised from 1,986 by `reason` (ADR 0750): a ladder \
             edit prepares the owner's card, which quotes it, and the agent \
             writes it while writing the call",
        ),
        (
            "navigate_ui",
            2_848,
            "the two frozen enums the SDK is generated from (18 targets, 24 \
             settings views) are about 460 chars before a word of prose; the settings \
             gloss is routing information available on no other surface, and \
             it names only the views whose own value does not (`mcp` is not \
             glossed, because the value already says what the page is). Raised \
             from 2,300 by `fragment`, the companion arg naming a place inside \
             the app. It has to say where the value surfaces, the app's own \
             `location.hash`, because no other surface the model reads says \
             so, and the choice is made while writing the call. Raised from \
             2,436 by `system-overview`: Settings > System became a submenu, \
             so `system` opens a list of rows and the versions and restart \
             the model used to reach there now need their own value. Raised \
             from 2,494 by `device`: a navigate reaches one device, and the \
             user asking from the phone to open it on the laptop is chosen \
             while writing the call. Raised from 2,595 by the `dashboard` \
             target (ADR 0432), the one value that lands a dashboard. Raised \
             from 2,607 by `intelligence` (ADR 0525): the ladders left Models \
             and Coding Agents for a page of their own. Raised from 2,621 by \
             `coding_agent` and `folder`: a new chat preselects its \
             destination, and the model chooses both while writing the call. \
             Raised from 2,822 by `providers` and the settings use of \
             `fragment`: a link to one provider's block is chosen while \
             writing the call. Raised from 2,839 by `people`: People left the \
             Devices page, where `devices` used to reach it",
        ),
        (
            "request_credential",
            2_434,
            "thirteen properties, seven of them the oauth_client endpoint set, \
             each already one line pointing at system-knowhow/oauth-providers. \
             Raised from 2,150 by the `secret` type: it adds a sixth enum \
             value, and the scope argument stays outside `required` because a \
             secret declares no scope, so this schema is the only surface \
             saying which types still need one. Raised again from 2,200 by \
             `base_urls`: an array schema plus the sentence saying a credential \
             names every host it reaches, and that naming a missed host widens \
             the existing one. Knowhow cannot carry it, since the argument is \
             what the model writes, and the failure it prevents is storing one \
             token under two service names. Raised from 2,351 by the builtin \
             proxy refusal: the agent must know before calling that a service \
             a configured builtin serves is refused, and which overrides proceed",
        ),
        (
            "ask_user_question",
            2_309,
            "the \"Other\"-option ban is deliberately mirrored between \
             ASK_USER_QUESTION_RULE and this schema, and pinned by a test in \
             each place; the nested question / options / label object is 434 \
             chars of frozen shape. Raised from 1,950 by `message`: a tool input \
             is never summarized, so it is the one place an always-thinking \
             model can hand the user an answer before a card. Raised by 135 \
             for `quiet`, the one way to offer a pick that ends the turn. Raised by \
             92 for `quiet_reply`, the fixed answer a quiet pick shows",
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
            4_114,
            "five more actions on a domain whose two existing schemas \
             are almost entirely a spelled-out `status` enum pinned to \
             `ThreadStatus::ALL` by a test, repeated across both because \
             `llm_schema` is a const JSON literal and cannot compose. `search` \
             adds its own summary, `(requires: q)` and two properties, \
             `detach_child` its summary and a `thread_id` (ADR 0278), and \
             `archive` the same again (ADR 0310). Raised from 2,243 for \
             `drafts` and `held_messages`, which reuse the union's properties, \
             and the two filters `has_draft` and `has_diff`. Raised from \
             2,737 for `triage` and `apply_triage` (ADR 0349), two short \
             summaries and one `entries` array. Raised from 3,159 for \
             `change_state`, the three-value enum that replaced `has_diff` \
             (ADR 0400). Raised from 3,172 for `cache_keep_alive`, one clause \
             and a two-value `state` enum (ADR 0554). Raised from 3,369 for \
             `context_ceiling`, one cut clause and a `ceiling` string, the \
             prompt-first path to the Settings ceiling (ADR 0629). Raised \
             from 3,614 for `submit_draft`, a summary and three integer or \
             uuid properties, the prompt-first path to sending a saved draft \
             (ADR 0651)",
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
                "no email account, image provider on",
                ToolCapabilities {
                    email_account: false,
                    image_provider: true,
                    judgment_provider: false,
                    context_mode: false,
                    memory_tree: false,
                },
            ),
            ("no capability at all", ToolCapabilities::default()),
        ] {
            let (n, chars) = wire(&caps);
            crate::log_legacy!("{label}: {n} schemas, {chars} wire chars");
        }
    }

    #[test]
    #[ignore]
    fn print_full_tool_schema_ranking() {
        crate::log_legacy!(
            "{}\n\n{}",
            full_ranking("flat tool schemas", &flat_chat_tools()),
            full_ranking(
                "grouped manifest tool schemas",
                &crate::capability_manifest::llm_tools()
            ),
        );
    }

    #[test]
    #[ignore]
    fn print_frozen_tool_contract() {
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
        tools.sort_by(|a, b| a.name.cmp(&b.name));

        for tool in &tools {
            crate::log_legacy!(
                "{}\t{}",
                tool.name,
                serde_json::to_string(&strip(&tool.parameters)).expect("schema serializes")
            );
        }
    }

    #[test]
    fn the_trigger_rules_ride_in_the_static_body_of_every_turn() {
        for source in [true, false] {
            for memory_tree in [true, false] {
                let body =
                    static_prompt_body(source, 500, CHILD_CAP, ContextMode::Off, memory_tree, "");
                assert!(
                    body.contains(crate::scheduler::user_tasks::TRIGGER_SYSTEM_ADDENDUM),
                    "source {source}, memory tree {memory_tree}"
                );
            }
        }
    }

    #[test]
    fn mandatory_setup_asks_for_technical_literacy_with_storable_values() {
        use crate::core::technical_literacy::SETTABLE_IDS;

        let (pref, instruction) = MANDATORY_PREFS
            .iter()
            .find(|(pref, _)| pref.spec().key == prefs::TECHNICAL_LITERACY.key())
            .expect("technical literacy is a mandatory preference");
        assert_eq!(
            pref.spec().scope,
            PrefScope::Global,
            "the level is workspace-global"
        );
        for id in SETTABLE_IDS {
            assert!(
                instruction.contains(&format!("\"{id}\"")),
                "the setup instruction never offers '{id}'"
            );
        }
        assert!(
            !instruction.contains("everyday"),
            "the merged level is never offered"
        );
        for level in crate::core::technical_literacy::TechnicalLiteracy::ALL {
            let option = format!("\"{}\" / \"{}\"", level.card_label(), level.card_line());
            assert!(instruction.contains(&option), "the card lacks {option}");
        }
    }

    #[test]
    fn only_a_turn_sent_from_a_device_enters_setup_mode() {
        assert_eq!(setup_device(false, Some("laptop")), Some("laptop"));
        assert_eq!(setup_device(true, Some("laptop")), None, "a trigger fire");
        assert_eq!(setup_device(false, None), None, "a sub-thread or task");
        assert_eq!(setup_device(true, None), None);

        let missing = ["- TECHNICAL LITERACY: ask"];
        let chat = setup_or_language_section(&missing, "English");
        assert!(chat.starts_with("SETUP REQUIRED"));
        assert!(chat.contains("TECHNICAL LITERACY"));

        let configured = setup_or_language_section(&[], "English");
        assert!(!configured.contains("SETUP REQUIRED"));
        assert!(configured.contains("Always respond in English."));
    }

    #[test]
    fn a_style_naming_a_placeholder_reaches_the_body_verbatim() {
        let style = "\nRESPONSE STYLE: cite __MAX_TOOL_CALLS__ and \
                     __MAX_CONCURRENT_CHILDREN_PER_THREAD__ as typed";
        let body = static_prompt_body(false, 777, CHILD_CAP, ContextMode::Off, false, style);

        assert!(
            body.contains(
                "cite __MAX_TOOL_CALLS__ and __MAX_CONCURRENT_CHILDREN_PER_THREAD__ as typed"
            ),
            "a placeholder in the style text was rewritten by a later pass"
        );
    }

    async fn mock_engine(db: &str, workspace: &Path) -> crate::engine::LucidosEngine {
        crate::engine::LucidosEngine::new(
            workspace.to_path_buf(),
            &crate::test_support::test_db_url(db),
            crate::llm::ProviderBuildContext {
                default_model: "mock".into(),
                model_is_mock: true,
                vertex_project_id: String::new(),
                vertex_location: crate::llm::vertex::location_handle(String::new()),
                vertex_token_cache: None,
                model_registry: Default::default(),
                not_served: Default::default(),
                claude_gateway_access: None,
                chatgpt_plan_access: None,
            },
            std::sync::Arc::new(crate::engine::agent_recovery::EngineAgentHost),
        )
        .await
        .unwrap()
    }

    #[tokio::test]
    async fn an_unreadable_preference_is_configured_not_setup_required() {
        use super::SystemPromptExt;
        use crate::engine::tools::capabilities::CapabilitiesExt;
        let (pool, db) = crate::test_support::setup_test_db().await;
        let workspace = tempfile::tempdir().unwrap();
        let engine = mock_engine(&db, workspace.path()).await;
        let capabilities = engine.read_turn_capabilities().await;

        let (fresh, fresh_missing) = engine
            .build_chat_system_prompt(
                "Europe/Oslo",
                "English",
                Some("laptop"),
                &None,
                500,
                &capabilities,
            )
            .await;
        assert!(
            fresh.contains("SETUP REQUIRED"),
            "a fresh workspace is unset"
        );
        assert!(!fresh_missing.is_empty());

        engine.pool.close().await;
        let (prompt, missing) = engine
            .build_chat_system_prompt(
                "Europe/Oslo",
                "English",
                Some("laptop"),
                &None,
                500,
                &capabilities,
            )
            .await;
        assert!(
            !prompt.contains("SETUP REQUIRED"),
            "an unreadable preference refused the turn"
        );
        assert!(missing.is_empty());

        pool.close().await;
        crate::test_support::teardown_test_db(&db).await;
    }

    #[tokio::test]
    async fn the_built_prompt_carries_no_per_turn_reading() {
        use super::SystemPromptExt;
        use crate::engine::tools::capabilities::CapabilitiesExt;
        let (pool, db) = crate::test_support::setup_test_db().await;
        let workspace = tempfile::tempdir().unwrap();
        let engine = mock_engine(&db, workspace.path()).await;
        let capabilities = engine.read_turn_capabilities().await;

        let (prompt, _) = engine
            .build_chat_system_prompt(
                "Europe/Oslo",
                "English",
                Some("laptop"),
                &None,
                500,
                &capabilities,
            )
            .await;

        for reading in [
            "[END CURRENT TIME]",
            "[END ENGINE BUILD]",
            "[END CLIENT URL]",
        ] {
            assert!(
                !prompt.contains(reading),
                "the cached system prompt carries a per-turn reading ({reading})"
            );
        }
        assert!(prompt.contains("[CURRENT TIME] block at the END"));
        assert!(prompt.contains("[CLIENT URL] block at the END"));

        pool.close().await;
        crate::test_support::teardown_test_db(&db).await;
    }

    #[test]
    fn the_standard_body_is_byte_identical_to_a_build_with_no_style() {
        let standard = static_prompt_body(false, 500, CHILD_CAP, ContextMode::Off, false, "");

        assert!(!standard.contains("RESPONSE STYLE:"));
        assert!(!standard.contains("__RESPONSE_STYLE_RULE__"));
        assert!(standard.contains(
            "- Create artifacts as the conversation progresses, not all at once.\n\nCOPYABLE TEXT:"
        ));
    }

    #[test]
    fn a_chosen_style_adds_exactly_its_own_section() {
        let section = crate::core::response_style::widest_shipped_section();
        let standard = static_prompt_body(false, 500, CHILD_CAP, ContextMode::Off, false, "");
        let styled = static_prompt_body(false, 500, CHILD_CAP, ContextMode::Off, false, &section);

        assert_eq!(styled.len(), standard.len() + section.len());
        assert!(styled.contains("RESPONSE STYLE:"));
        assert!(styled.contains(&format!(
            "- Create artifacts as the conversation progresses, not all at once.{section}"
        )));
    }

    #[test]
    fn each_shipped_style_splices_its_own_block_and_no_others() {
        use crate::core::response_style;

        let library = response_style::merge(&[]);
        for style in &library {
            let section = response_style::section_for(&library, &style.id, None);
            let body = static_prompt_body(false, 500, CHILD_CAP, ContextMode::Off, false, &section);
            if style.id == response_style::STANDARD_ID {
                assert!(!body.contains("RESPONSE STYLE:"));
                continue;
            }
            assert!(
                body.contains(&style.instruction),
                "{} did not reach the prompt",
                style.id
            );
            for other in &library {
                if other.id == style.id || other.instruction.is_empty() {
                    continue;
                }
                assert!(
                    !body.contains(&other.instruction),
                    "{} leaked into the {} prompt",
                    other.id,
                    style.id
                );
            }
        }
    }

    #[test]
    fn the_mode_on_body_points_at_the_todo_heading_rather_than_the_tool() {
        let off = static_prompt_body(false, 500, CHILD_CAP, ContextMode::Off, false, "");
        let on = static_prompt_body(false, 500, CHILD_CAP, ContextMode::On, false, "");

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

    #[test]
    fn the_browser_section_prefers_a_proxy_or_mcp_tool_and_says_when_it_falls_back() {
        let body = static_prompt_body(false, 500, CHILD_CAP, ContextMode::Off, false, "");
        let from = body
            .find("BROWSER TOOLS:\n")
            .expect("the prompt no longer has a BROWSER TOOLS section");
        let section = &body[from..];
        let section = &section[..section.find("\n\n").unwrap_or(section.len())];

        assert!(
            section.contains("proxy_request") && section.contains("an MCP tool"),
            "the section must name the configured proxy and MCP tools as the \
             first choice over the browser:\n{section}"
        );
        assert!(
            section.contains("only when neither covers the task, and say so"),
            "the browser is the fallback, and the user must hear when it is \
             used:\n{section}"
        );
    }

    #[test]
    fn the_prompt_names_the_engine_host_and_parts_it_from_the_users_device() {
        let body = static_prompt_body(false, 500, CHILD_CAP, ContextMode::Off, false, "");
        let from = body
            .find("ENGINE HOST:")
            .expect("the prompt no longer has an ENGINE HOST section");
        let section = &body[from..];
        let section = &section[..section.find("\n\n").unwrap_or(section.len())];

        let host = format!("`{}` (`{}`)", std::env::consts::OS, std::env::consts::ARCH);
        assert!(
            section.contains(&host),
            "the section must name this engine's own OS and arch:\n{section}"
        );
        assert!(
            section.contains("need not be the user's device"),
            "the section must part the host from the user's device:\n{section}"
        );
    }

    #[test]
    fn the_fan_out_section_says_a_direct_child_reports_back_on_its_own() {
        let body = static_prompt_body(false, 500, CHILD_CAP, ContextMode::Off, false, "");
        let from = body
            .find("PARALLEL WORK (FAN-OUT):\n")
            .expect("the prompt no longer has a FAN-OUT section");
        let section = &body[from..];
        let section = &section[..section.find("\n\n").unwrap_or(section.len())];

        for needle in [
            "A DIRECT CHILD REPORTS BACK ON ITS OWN",
            "cut by an engine crash",
            "END YOUR TURN",
            "NEVER `await_event` ON YOUR OWN CHILD",
            "`CodingAgentIdled`, `ResponseGenerated` or `ChildThreadCompleted`",
            "`interrupted`",
            "a grandchild reports to your child, not you",
            "'wake' re-opens it for any report",
            "The spawn's `wake_mode` decides",
            "'no-wake' FILES every report for your next turn",
            "Default: 'no-wake' from the home thread, 'wake' elsewhere",
            "The mode lasts the child's life",
            "WON'T ACT ON THE RESULT? relation='top'",
        ] {
            assert!(
                section.contains(needle),
                "the FAN-OUT section must carry {needle:?}:\n{section}"
            );
        }
    }

    #[test]
    fn the_batch_rule_is_general_and_keeps_both_carve_outs() {
        let body = static_prompt_body(false, 500, CHILD_CAP, ContextMode::Off, false, "");

        assert!(
            body.contains("Independent tool calls go IN THE SAME RESPONSE"),
            "the batch rule must cover every independent call, not only file writes"
        );
        assert!(
            body.contains("Only a call that needs an earlier result waits for it."),
            "the batch rule must say which calls do not batch"
        );
        assert!(
            body.contains("NEVER parallel-call it alongside other tools"),
            "a question card must never share a response with another call"
        );
        assert!(
            body.contains("Never batch sequential spawns into one response."),
            "a spawn that depends on the one before it must go alone"
        );
    }

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
        assert!(
            rule.contains("triggers(action=\"run\")"),
            "the rule must name the action that runs a trigger off-schedule:\n{rule}"
        );
        assert!(
            rule.contains("resuming a paused trigger does not run it"),
            "must pre-empt the resume-means-run guess:\n{rule}"
        );
        assert!(
            !rule.contains("action create/update"),
            "the create/update-only phrasing is what caused the miss:\n{rule}"
        );
    }

    #[test]
    fn workspace_assets_rule_routes_existing_plugin_operations() {
        let rule = WORKSPACE_ASSETS_KNOWHOW_RULE;

        assert!(
            rule.contains("EXISTING trigger or plugin"),
            "the rule must fire for an existing plugin, not only a create:\n{rule}"
        );
        let plugin_row = rule
            .lines()
            .find(|line| line.contains("`system-knowhow/plugins`"))
            .expect("the rule must route plugins to system-knowhow/plugins");
        for action in ["ANY action", "publish", "update"] {
            assert!(
                plugin_row.contains(action),
                "the plugin row must cover {action:?}, not only packaging:\n{plugin_row}"
            );
        }
    }

    #[test]
    fn setup_interview_rule_carves_itself_out_of_action_first() {
        assert!(
            SETUP_INTERVIEW_RULE.contains("ACTION FIRST rule below does NOT apply"),
            "the interview must except itself from ACTION FIRST, or the agent \
             skips the questions:\n{SETUP_INTERVIEW_RULE}"
        );
    }

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
        assert!(
            section.contains("NEVER say or imply you have read"),
            "must forbid claiming to have read the source:\n{section}"
        );
        assert!(
            section.contains("NEVER tell the user to Apply, rebuild, or restart"),
            "must forbid the apply/rebuild instruction:\n{section}"
        );
        assert!(
            section.contains("data/apps/<id>") && section.contains("manage_repositories"),
            "must name the app + external-repo routes that still work:\n{section}"
        );
    }

    #[test]
    fn no_source_variant_routes_platform_work_to_another_install_via_the_cli() {
        let section = coding_surface_section(false);

        assert!(
            section.contains("ANOTHER INSTALL CAN"),
            "must keep the route to another install open:\n{section}"
        );
        assert!(
            section.contains("lucidos spawn-thread --to <absolute path>")
                && section.contains("--coding-agent claude-code"),
            "must show the CLI call shape that works:\n{section}"
        );
        assert!(
            section.contains("run_coding_agent(workspace=…)` CANNOT reach it"),
            "must say the tool cannot cross installs, or the agent tries it \
             first:\n{section}"
        );
        assert!(
            section.contains("every workspace this install serves")
                && section.contains("local coding agent"),
            "the refusal must be scoped to this install so it doesn't read as \
             a blanket ban:\n{section}"
        );
    }

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

    #[test]
    fn the_two_variants_are_distinct() {
        assert_ne!(
            coding_surface_section(true),
            coding_surface_section(false),
            "the source and no-source prompt variants must differ"
        );
    }

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

    #[test]
    fn the_look_rule_bans_a_thread_verdict_from_history() {
        let rule = LOOK_BEFORE_ASSESSING_RULE;
        for needle in [
            "In EVERY reply, not only when asked to judge",
            "only from a read in THIS turn",
            "the [CHILD THREADS] block",
            "say it in the past tense with its time",
            "its earlier reports are history",
            "Read what a thread is doing from liveness.state",
            "status: idle means no turn is open",
            "A waiting thread wakes itself on the waits liveness names",
        ] {
            assert!(rule.contains(needle), "missing `{needle}`:\n{rule}");
        }
        assert!(
            rule.find("In EVERY reply").unwrap() > rule.find("stop once you have").unwrap(),
            "the verdict rule closes the section, outside the assessment bullets:\n{rule}"
        );
    }

    #[test]
    fn the_look_rule_names_the_two_searches_and_bounds_them() {
        assert!(
            LOOK_BEFORE_ASSESSING_RULE.contains("__MEMORY_LOOKUP__"),
            "it must name what to look with:\n{LOOK_BEFORE_ASSESSING_RULE}"
        );
        assert!(
            MEMORY_LOOKUP_CLASSIC.contains("`memory` and `threads` tools both have a 'search'"),
            "{MEMORY_LOOKUP_CLASSIC}"
        );
        assert!(
            MEMORY_LOOKUP_TREE.contains("`recall` and `threads` tools both have a 'search'"),
            "{MEMORY_LOOKUP_TREE}"
        );
        assert!(
            LOOK_BEFORE_ASSESSING_RULE.contains("not browsing"),
            "the user's constraint (only when necessary) must ride with the \
             permission:\n{LOOK_BEFORE_ASSESSING_RULE}"
        );
    }

    #[test]
    fn a_tree_body_names_recall_and_never_the_memory_tool() {
        let classic = static_prompt_body(false, 500, CHILD_CAP, ContextMode::Off, false, "");
        let tree = static_prompt_body(false, 500, CHILD_CAP, ContextMode::Off, true, "");

        assert!(classic.contains(MEMORY_LOOKUP_CLASSIC));
        assert!(classic.contains(MEMORY_CORRECTIONS_RULE_CLASSIC));
        assert!(tree.contains(MEMORY_LOOKUP_TREE));
        assert!(tree.contains(MEMORY_CORRECTIONS_RULE_TREE));
        for gone in ["the `memory` tool", "`memory` and `threads`"] {
            assert!(!tree.contains(gone), "a Tree body still says {gone:?}");
        }

        let swapped = classic
            .replace(MEMORY_LOOKUP_CLASSIC, MEMORY_LOOKUP_TREE)
            .replace(
                MEMORY_CORRECTIONS_RULE_CLASSIC,
                MEMORY_CORRECTIONS_RULE_TREE,
            );
        assert_eq!(
            swapped, tree,
            "the module must move nothing but its two sections"
        );
    }

    #[test]
    fn workspace_line_abbreviates_home_and_leaks_no_absolute_home_path() {
        let home = std::env::var("HOME").expect("HOME must be set to run this test");
        let workspace = PathBuf::from(&home).join("workspaces/myws");

        let timezone = super::super::turn_clock::timezone_section("Europe/Oslo");
        let section =
            workspace_identity_section("myws", &workspace, &timezone, "USER LANGUAGE: English");

        assert!(section.contains("WORKSPACE: myws (~/workspaces/myws)"));
        assert!(
            !section.contains(&home),
            "identity section still carries the home-rooted absolute path {home}:\n{section}"
        );

        assert!(section.contains("USER TIMEZONE: Europe/Oslo"));
        assert!(section.contains("TIMEZONE HANDLING:"));
        assert!(section.contains("USER LANGUAGE: English"));
        assert!(section.contains("PERSONAL DATA ACCESS:"));

        assert!(
            !section.contains("CURRENT TIME:"),
            "the cached identity section must carry no clock reading:\n{section}"
        );
    }

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
        assert!(
            rule.contains("thread_title") && rule.contains("its subject line, never the sha"),
            "rule must name the human-readable substitute for each thing:\n{rule}"
        );
        assert!(
            rule.contains("changes(action='apply', change_id="),
            "rule must keep ids legal in tool arguments:\n{rule}"
        );
        assert!(
            rule.contains("A MARKDOWN LINK TARGET IS NOT PROSE")
                && rule.contains("[Open thread](thread:<ws>/<uuid>)"),
            "rule must exempt markdown link targets so spawned-thread links survive:\n{rule}"
        );
        assert!(
            rule.contains("(trigger:<id>)") && rule.contains("The NAME goes in the label"),
            "rule must exempt the trigger link form and place the name in the label:\n{rule}"
        );
    }

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
        assert!(
            rule.contains("do not go looking for the edge it misses"),
            "rule must not read as an invitation to find the engine's gap:\n{rule}"
        );
    }

    #[test]
    fn no_impersonation_rule_does_not_call_cross_workspace_impossible() {
        let rule = NO_IMPERSONATION_RULE;
        assert!(
            !rule.contains("acting in another workspace"),
            "cross-workspace is a supported route, not a refusal case:\n{rule}"
        );
        assert!(
            rule.contains("lucidos spawn-thread --to"),
            "rule must name the route that works, or the agent invents a \
             workaround for a capability it has:\n{rule}"
        );
    }

    #[test]
    fn the_chat_rule_routes_the_answer_into_the_card_message() {
        assert!(ASK_USER_QUESTION_RULE.contains("PUT THE ANSWER IN ITS `message`"));
        assert!(ASK_USER_QUESTION_RULE.contains("short summary"));
        assert!(ASK_USER_QUESTION_RULE.contains("links, pictures and drafts"));
    }

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
        assert!(
            !rule.contains("Always include an option that lets the user opt out"),
            "the retired opt-out instruction must not reappear in the chat rule:\n{rule}"
        );
        assert!(
            rule.contains("None of these") && rule.contains("still welcome"),
            "rule must keep a meaningful opt-out option legal:\n{rule}"
        );
    }

    #[test]
    fn workspace_outside_home_keeps_its_absolute_path() {
        let outside = Path::new("/srv/lucidos/ws");
        let home = std::env::var("HOME").expect("HOME must be set to run this test");
        assert!(!outside.starts_with(&home), "test fixture overlaps $HOME");

        let section = workspace_identity_section("shared", outside, "", "");
        assert!(section.contains("WORKSPACE: shared (/srv/lucidos/ws)"));
    }

    #[test]
    fn the_trigger_rule_has_the_engine_review_a_yes() {
        let rule = SYSTEM_PROMPT_BASE
            .lines()
            .find(|l| l.starts_with("- In a trigger run"))
            .expect("the TRIGGERS block states the trigger-run rule");
        for must in [
            "false for routine reports or after a notification",
            "a failure the notification omitted",
            "The engine reviews a yes.",
        ] {
            assert!(rule.contains(must), "missing {must:?}:\n{rule}");
        }
        assert!(
            !SYSTEM_PROMPT_BASE.contains("just finish"),
            "\"just finish\" leaves the decision to the forced call"
        );
        assert!(
            !rule.contains("__"),
            "the cached prefix holds no per-run data"
        );
    }

    #[test]
    fn trigger_vs_event_wait_rule_routes_on_destination_without_overclaiming() {
        let rule = TRIGGER_VS_EVENT_WAIT_RULE;

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

        assert!(
            rule.contains("one-shot") && rule.contains("outlive this conversation"),
            "dropping the duration half routes standing rules into a wait that \
             stops after one match:\n{rule}"
        );

        assert!(
            rule.contains("await_event") && rule.contains("trigger"),
            "a fork has to name both branches:\n{rule}"
        );

        assert!(
            rule.contains("reaches the user as a notification"),
            "must say what a trigger DOES do, or it reads as 'triggers cannot \
             tell me anything':\n{rule}"
        );

        assert!(
            rule.starts_with("\"TELL ME WHEN X HAPPENS\""),
            "the state-change section names this block by its opening heading; \
             renaming it here silently dangles that pointer:\n{rule}"
        );
    }

    #[test]
    fn the_body_states_the_child_cap_it_was_given() {
        let body = static_prompt_body(false, 500, 4, ContextMode::Off, false, "");
        assert!(
            body.contains("At most 4 children running at the same time"),
            "the prompt must name the policy's cap as a limit at one moment"
        );
        assert!(!body.contains("__MAX_CONCURRENT_CHILDREN_PER_THREAD__"));
    }

    #[test]
    fn the_body_tells_agents_to_collect_lucidos_logs_themselves() {
        let body = static_prompt_body(false, 500, 4, ContextMode::Off, false, "");
        let sandbox = body
            .lines()
            .find(|line| line.starts_with("AGENT SANDBOX:"))
            .expect("the prompt has a sandbox section");
        assert!(sandbox.contains("run `lucidos diagnostic-log query` yourself"));
        assert!(sandbox.contains("Never give the owner Terminal commands for Lucidos logs."));
    }

    mod guard_tests {
        use super::*;

        fn always_loaded_areas() -> Vec<(&'static str, usize)> {
            let repo = crate::paths::repo_root().expect("repo root resolves under cargo test");
            let ws = tempfile::tempdir().expect("temp workspace");
            let built_in_widgets = crate::core::AppManager::new(ws.path())
                .expect("app manager")
                .with_system_widgets(Some(repo.join("system-widgets")))
                .list_reusable_widgets()
                .expect("built-in widgets load");
            system_prompt_footprint(
                &BILLED,
                &crate::core::SystemKnowhowStore::load_summaries(&repo.join("system-knowhow")),
                &built_in_widgets,
            )
        }

        #[test]
        fn always_loaded_context_stays_under_budget() {
            let areas = always_loaded_areas();
            let total: usize = areas.iter().map(|(_, n)| n).sum();

            let breakdown = areas
                .iter()
                .map(|(name, n)| format!("  {n:>7} chars  {name}"))
                .collect::<Vec<_>>()
                .join("\n");
            crate::log_legacy!(
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

        #[test]
        fn every_knowhow_id_the_resident_context_routes_to_resolves() {
            let repo = crate::paths::repo_root().expect("repo root resolves under cargo test");

            let mut haystack = format!(
                "{}{}{}",
                static_prompt_body(true, 500, CHILD_CAP, ContextMode::Off, false, ""),
                static_prompt_body(false, 500, CHILD_CAP, ContextMode::Off, false, ""),
                static_prompt_body(true, 500, CHILD_CAP, ContextMode::On, false, "")
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

        #[test]
        fn every_prompt_placeholder_is_substituted() {
            let repo = crate::paths::repo_root().expect("repo root resolves under cargo test");
            let path =
                repo.join("crates/lucidos-agents-api/src/engine/chat/process/system_prompt.rs");
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

        #[test]
        fn the_state_change_section_excludes_the_threads_own_child() {
            let repo = crate::paths::repo_root().expect("repo root resolves under cargo test");
            let path =
                repo.join("crates/lucidos-agents-api/src/engine/chat/process/system_prompt.rs");
            let src = std::fs::read_to_string(&path)
                .unwrap_or_else(|e| panic!("read {}: {e}", path.display()));

            let heading = "WAITING FOR A STATE CHANGE IN LUCIDOS";
            let from = src
                .find(heading)
                .unwrap_or_else(|| panic!("the prompt no longer has a '{heading}' section"));
            let section = &src[from..];
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

            assert!(
                section.contains("PRECONDITION, NOT A GO-AHEAD"),
                "a 'when X finishes' clause is something to wait for; a \
             pre-approval attached to it covers the action once X has happened \
             rather than replacing X:\n{section}"
            );
        }

        fn file_references_section() -> String {
            let repo = crate::paths::repo_root().expect("repo root resolves under cargo test");
            let path =
                repo.join("crates/lucidos-agents-api/src/engine/chat/process/system_prompt.rs");
            let src = std::fs::read_to_string(&path)
                .unwrap_or_else(|e| panic!("read {}: {e}", path.display()));

            let from = src
                .find("FILE REFERENCES:\n")
                .expect("the prompt no longer has a FILE REFERENCES section");
            let section = &src[from..];
            section[..section.find("\n\n").unwrap_or(section.len())].to_string()
        }

        #[test]
        fn the_file_references_section_links_a_trigger_not_just_the_panel() {
            let section = file_references_section();
            let section = section.as_str();

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
            assert!(
                section.contains("Run once"),
                "the section must name what the trigger link lands on:\n{section}"
            );
        }

        #[test]
        fn the_file_references_section_says_an_image_is_drawn_not_linked() {
            let section = file_references_section();
            let section = section.as_str();

            assert!(
                section.contains("![what it shows](artifacts/branding/card.png)"),
                "the section must show the image form, not just name it:\n{section}"
            );
            assert!(
                section.contains("is a LINK"),
                "the section must tell a link apart from a drawn image:\n{section}"
            );
            assert!(
                section.contains("shows it to YOU"),
                "the section must say a read reaches the agent, not the user:\n{section}"
            );
            assert!(
                section.contains("Saving one shows nothing either"),
                "the section must say a saved image reaches nobody:\n{section}"
            );
            assert!(
                section.contains("in an option's description"),
                "the section must put pictures on the question card:\n{section}"
            );
            assert!(
                section.contains("text that ENDS your turn")
                    && section.contains("the card's `message`")
                    && section.contains("followed by any tool call"),
                "the section must name every place a picture shows:\n{section}"
            );
            assert!(
                !section.contains("draw it in your reply"),
                "a reply followed by a tool call loses the picture:\n{section}"
            );
            assert!(
                section.contains("never ASCII art")
                    && section.contains("its own picture of only that option"),
                "the section must show a visual choice as one picture per option:\n{section}"
            );
            assert!(
                section.contains("only that option, in its description, not in `message`"),
                "an option's picture must go on its own option:\n{section}"
            );
        }

        #[test]
        fn the_file_references_section_links_inside_an_app_not_only_at_it() {
            let section = file_references_section();
            let section = section.as_str();

            assert!(
                section.contains("(app:pr-understanding#pr-1645)"),
                "the section must show the fragment form, not just name it:\n{section}"
            );
            assert!(
                section.contains("location.hash"),
                "the section must say how the fragment reaches the app:\n{section}"
            );
            assert!(
                section.contains("when the app reads one"),
                "the section must gate the fragment on the app reading it:\n{section}"
            );
        }

        #[test]
        fn workspace_assets_rule_names_only_live_knowhow_ids() {
            let rule = WORKSPACE_ASSETS_KNOWHOW_RULE;
            let repo = crate::paths::repo_root().expect("repo root resolves under cargo test");

            for id in [
                "triggers",
                "building-an-app",
                "building-knowhow",
                "plugins",
                "event-hooks",
            ] {
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
    }
}
