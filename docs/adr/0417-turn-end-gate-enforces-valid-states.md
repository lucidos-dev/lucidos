# 0417: One turn-end gate enforces valid states, starting with the read decision

- **Status**: Accepted
- **Date**: 2026-10-10
- **Amends**: [0409](0409-review-lists-read-requests.md)

## Context

ADR 0409 made the read request opt-in. In its first two hours live, 8 chat
threads and 8 coding-agent threads finished a turn and none asked to be read.
In one, the user said "leave just a message for me", and the agent replied
with findings and never asked. The tool was offered on every round.

Silence was a valid answer, and nothing checked the state a turn ended in.
Lucidos already wanted a turn to end valid in two other ways, each with its
own mechanism and reach:

- a Lucidos-source branch carries a plan marker: an engine re-entry after
  idle, on both backends;
- committed work is hardened: a soft block in the Claude Code Stop hook, on
  Claude Code only.

## Decision

- **One turn-end gate.** `engine/turn_end/` holds every *turn-end
  requirement*. Both turn ends, chat and coding agent, call it: the read
  decision just before the turn's response is recorded, and a coding agent's
  re-entry just after its idle.
- **Every turn makes a read decision, with no default.** `request_read` takes
  a required `read` boolean, and `lucidos request-read` takes `yes` or `no`.
  A no records `ThreadReadNotRequested`, which projects nothing.
- **A missing decision is forced, on an auxiliary model.** For every thread
  kind, the engine makes one call under its own background purpose, with
  `model_read_decision` as its model preference. The call reads the turn's
  opening message and drafted reply, with `request_read` as the only tool,
  forced. The draft reply is never changed.
- **Work only the agent can do is a session re-entry.** A *proposal hold*
  (ADR 0416) sends a Lucidos-source coding agent back with its nudge: for a
  missing plan marker or a missing harden, each at most once per HEAD. The
  hold owns the rule and its text, and reaches both backends, so the Stop
  hook's harden reminder is removed.
- **A forced call that errors fails visible**: the engine records a read
  request in its own name.

## Rationale

A decision the model can skip is not a decision. Provider APIs can force one
tool call for exactly this. So the engine asks the question the turn did not
answer, instead of guessing.

One gate gives every valid-state rule one definition and one place to look.
Before, the plan floor lived in the session runner. The harden rule lived in
a hook only one backend runs, so no Codex session was asked to harden.

A forced decision fits a decision, and a re-entry fits work. The read
decision needs only judgement over text already written. A plan or a harden
run needs the agent's tools and worktree.

Failing towards a read request keeps the error visible: the thread lands in
Review, where the user looks anyway. Failing towards no would hide a report
silently.

## Consequences

- The chat agent decides only when it is sure, and the prompt does not ask
  every turn. On always-thinking Claude models and on Gemini, text beside a
  tool call is not shown as the reply. Asking every turn cost a second full
  round. An undecided turn costs one auxiliary call, recorded as a
  `read_decision` capture. A reply that carries its own decision beside its text costs no
  extra call, and ends the turn without another round.
- `ModelSelection` gains `forced_tool`, and every provider maps it to its
  own `tool_choice` form. It serves engine calls only. A Claude model that
  always thinks refuses any forced tool, so the provider refuses it before
  sending, and the gate records a read request.
- A no never clears an earlier unseen request, and never marks a thread as
  needing attention. A trigger run that decides no stays archived.
- The Stop hook keeps its plaintext question redirect. Apply's synchronous
  hardening stays as the floor.
- The chat loop's question re-ask and wake check stay in the loop, outside
  the gate. They depend on loop state mid-turn.

## Alternatives considered

- **A stronger prompt only.** Rejected: the prompt and the tool description
  already said when to ask, and no thread asked. Guidance had failed.
- **A nudge loop that sends the agent back until it decides.** Rejected: it
  costs a full turn and can loop. One forced call answers in one round.
- **A Stop hook per requirement.** Rejected: hooks run on Claude Code only,
  so Codex would keep missing every rule, and each hook would hold its own
  copy of the rule.
- **Force the call on the turn's own model.** Rejected during
  implementation. Opus 5.5, Sonnet 5.5 and Fable 5 always think and refuse a
  forced `tool_choice`, so every undecided turn on them would fail into
  Review. Offering one tool also changes the tools array, which drops a long
  conversation's whole prompt cache.
- **Default to no.** Rejected: that is ADR 0409's silence again, and reports
  stay hidden.
- **Default to yes.** Rejected: every acknowledgement would land in Review,
  and the group would stop meaning "your turn to look".
