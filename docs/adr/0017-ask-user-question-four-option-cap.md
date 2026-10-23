# 0017 — `ask_user_question` caps options at 4, inherited from Claude Code

- **Status** — Accepted
- **Date** — 2026-06-25

## Context

The `ask_user_question` tool (chat agent) and Claude Code's native
`AskUserQuestion` both render each question as a QuestionCard with the options as
clickable buttons. The schema caps a single question at **2–4 options**
(`minItems: 2, maxItems: 4`) and a single call at **1–4 questions**
(`crates/lucidos-engine/src/llm/tools/misc.rs`; mirrored in the engine prompt and
the CC-side MCP bridge `crates/lucidos-cli/src/mcp_permission_server.rs`).

The question came up after a thread where the model had 6 candidate phrasings
(A–F): it rendered A–D as buttons and parked E and F in the question *prose*
("…also on the table — just say the letter"). That looked like a bug ("why didn't
it spell out E–F?"); it is the model working around the 4-option ceiling. There
was no recorded rationale for the number, so re-litigating it ("why 4? bump it?")
had nothing to read. This ADR records the *why* so the next person doesn't have to
re-derive it.

## Decision

Keep the option cap at **4** (and the questions-per-call cap at 4). We did not
independently choose this number: the chat `ask_user_question` tool was built as a
deliberate, schema-identical clone of Claude Code's native `AskUserQuestion` (see
`docs/plans/2026-05-18-chat-ask-user-question-design.md`: *"Behaviour contract:
identical to CC's AskUserQuestion. Same JSON schema"*), and that upstream tool
caps options at 4. The cap rides along by design. We keep it rather than diverge.

## Rationale

- **One schema, two agents.** The whole point of the chat tool was to reuse CC's
  surface verbatim — same `UserQuestionAsked`/`UserQuestionAnswered` events, same
  `QuestionOption` struct, same `QuestionCard`, same parser. Diverging on
  `maxItems` would fork a contract whose value is that it *doesn't* fork.
- **It's a button card on a mobile-first PWA.** ~4 tappable buttons (plus the
  implicit free-text path) fit a phone card without scrolling/crowding. More
  buttons degrade into a cramped scrolling list — the opposite of "a tap beats
  typing."
- **Past ~4, multiple-choice stops winning.** The tool exists for cases where a
  button beats a typed reply. Beyond a handful of options the user is scanning a
  menu, at which point plain text — or splitting into two narrower questions — is
  genuinely the better surface.
- **Free text is always the escape hatch.** The 4 buttons are the *common*
  answers, never the *only* ones: the user can always type a custom reply, which
  the engine routes to the pending question as a `FreeText` answer. Overflow has a
  built-in release valve (in the motivating thread, E/F were reachable by typing
  the letter). **That escape is the prompt textarea, never an option.** This bullet
  originally also said "the prompt instructs the model to include an opt-out
  option". On 2026-08-04 that instruction was deleted from every question-tool
  surface and replaced with its opposite: the model must NOT author an "Other /
  Let me type it" option, because there is no text-entry option kind and picking
  one returns its *label* as the answer. A meaningful opt-out ("None of these") is
  still a legitimate option; a text-entry escape never was one. The cap itself is
  unaffected, and the escape hatch this bullet relies on is now named by the
  prompt textarea's placeholder while a question is pending ("Type custom answer
  here…"), which is the field the escape actually is. Cancel, the other escape
  that needs no option slot, is named by the prompt row's Cancel tooltip.

## Consequences

- A genuinely 5+-way mutually-exclusive choice cannot be expressed as one card.
  The model must narrow to the top 4 (free text absorbs the rest), split into ≤4
  sequential questions in one call, or group the choices into ≤4 buckets. When it
  instead stuffs overflow into the question prose, the result is the "A–D buttons,
  E–F in text" shape — usable but inelegant.
- The cap is enforced only by the tool **schema** (JSON-Schema `maxItems`), not by
  a runtime clamp; a provider that ignores `maxItems` is not defended against in
  code. This has not been a problem in practice.
- Raising it later is a three-site edit (`llm/tools/misc.rs`, the prompt text, and
  `lucidos-cli/src/mcp_permission_server.rs`) plus a frontend check that the
  QuestionCard still lays out acceptably on mobile — a product call about button
  crowding vs. flexibility, not a one-line constant bump.

## Alternatives considered

- **Raise the cap (e.g. 6–8 options).** Rejected for now. It buys flexibility for
  the rare wide-choice question at the cost of wider/scrolling cards on mobile and
  the choice-overload effect above, *and* forks the schema away from CC's. The
  free-text escape hatch already covers overflow, so the marginal value is low.
- **Add a prompt nudge to split/group instead of parking overflow in prose.**
  Considered as a complement, not a reason to change the cap. Reasonable to add if
  the "options in prose" shape recurs; deferred until it's a real pattern rather
  than a single instance.
- **Runtime-clamp to the first 4 options.** Rejected: it would silently drop
  options the model deliberately wrote, which is worse than the schema rejecting an
  over-long list. The schema constraint is the right enforcement point.
