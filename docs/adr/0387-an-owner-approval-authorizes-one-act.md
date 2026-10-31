# 0387: An owner's Allow on an engine-worded card authorizes one clause-4 act: amends 0168

- **Status**: Accepted
- **Date**: 2026-10-07
- **Amends**: [0168: A thread acts in its own subtree](0168-a-thread-acts-in-its-own-subtree.md)

## Context

ADR 0168 clause 5 lets a thread press the workspace owner's buttons only while
it carries a standing instruction: a turn the owner opened, or a trigger firing
they authorized. Nothing else counts, and nothing is inherited down the spawn
link.

That left a thread with no way to ask. A coding-agent thread spawned by another
thread was woken by an event-wait delivery. It asked the owner through
`AskUserQuestion` whether to spawn a separate thread, and the owner tapped
"Spawn a thread". `lucidos spawn-thread` was refused anyway, because the owner
had not opened that turn. Their tap was recorded as an answer, and an answer
was nobody's evidence.

Simply counting the answer is unsafe. The agent writes an ordinary card, so a
card reading "Keep going?" could lift the thread to the owner's full authority.
And an agent can answer its own card: the answer route gates on thread reach,
and a thread reaches itself.

## Decision

A thread may ask for an *owner approval* of one clause-4 act. The engine words
the card. The owner's **Allow once** from a registered device lets that thread
press that verb, at that target, once. The approval expires at the thread's
next turn start.

Asking takes two steps. `lucidos ask-owner-approval <verb> [--thread <id>]`
records the request and prints its id. The agent then asks `AskUserQuestion`
with that id as its only question, and the engine shows its own card in that
question's place.

Clause 5 keeps its two shapes. An owner approval is narrower than either: one
act, never the rest of the turn.

## Rationale

**The engine words what the owner taps.** The question names the verb and its
target, and both option labels are fixed. The agent's reason is quoted below
under its own label. So a leading question has nowhere to go: the agent writes
no part of what is approved.

**One act, verb plus target.** A yes to "create a top-thread" must not cover an
Apply elsewhere. A yes to "apply thread A's change" must not cover thread B's.
The owner approves what they saw, and nothing wider.

**The card rides the agent's own question tool.** A question card belongs to
the turn that asks it. When that turn ends, the UI strikes the card through,
recovery stops treating the thread as parked, and the status goes idle. Only
the question tools block for hours, so the approval card is one of their
cards. Badge, push, restart preservation and answer delivery all follow
unchanged.

**The MAC binds it to the thread.** The request route takes the thread from the
origin token, never from the body. So does the ask route that shows the card,
which finds the request only on that thread. The spend looks up approvals on
the thread the token proves. So a sibling cannot spend another thread's
approval, and no new token field is needed. The token has no per-turn nonce, so time is bounded
by the turn instead.

**Only a registered device's Allow counts.** The answer route refuses an
agent's answer to an approval card, and the spend reads the answer's actor kind
as `device` as well. "Don't allow", Canceled, Superseded and free text grant
nothing.

**Expiry rides the turn, read the ADR 0282 way.** An engine resume the owner
did not click is the same turn and keeps the approval. An event-wait wake
writes no turn start, so a thread waiting on the e2e lock keeps it too. A new
turn start ends it. No clock, which matches clause 6.

**Single use is the database's job.** A spend writes `OwnerApprovalSpent`, and
a partial unique index on its `tool_use_id` refuses a second one. The gate
spends before the handler writes, so a handler that fails has spent it. That is
the fail-closed direction.

**The recovery gates are untouched.** None of the card, its answer or the spend
is a turn start, so `THREAD_START_EVENTS_SQL` does not change. The approval's
lifetime reuses the standing instruction's current-turn read, so the two cannot
disagree about which turn is current.

**Two verbs stay off the card.** Answering a question card and resolving a
permission card are already the owner's own answer, on screen. A card that
authorized answering another card would put their voice on words they never
read. Unscoped cancel stays off too, since it names no target.

## Consequences

- A thread the owner did not open can still reach outside its subtree, one act
  at a time, with the owner's tap behind each act.
- An agent that skips the second step, or words its own card, gets an
  ordinary question card. Its answer grants nothing.
- `POST /api/v1/internal/ask-user-question` and its permission-prompt sibling
  now refuse a token naming another thread. Both took the thread from the body.
  So one thread could raise a card on another: a question that thread's agent
  never asked, or a permission whose "Allow for this thread" widened it.
- **The approval does not bind the request body.** The owner approves "create a
  top-thread", not that thread's prompt. Like ADR 0168's own gap, the engine
  checks the act and not its content.

## Alternatives considered

- **Count any owner answer as opening the turn.** Rejected. The agent writes an
  ordinary card, so one yes to anything would lift the thread to every clause-4
  verb, anywhere, until the next turn start.
- **Tag an `AskUserQuestion` option as approving a verb.** Rejected. Claude
  Code's built-in tool has a fixed schema, so the tag would ride a magic label,
  and the agent would still word the question. The chosen form passes only a
  key the engine minted, and the engine replaces every word of the card.
- **The engine asks when it refuses.** Rejected. Every refused attempt would
  raise a card, including an accidental unscoped cancel the owner might tap
  through.
- **A sub-thread inherits the standing instruction of the turn that spawned
  it.** Rejected, as ADR 0168 rejected spawner standing. The owner never read
  the prompt the sub-thread acts on, and a whole tree of spawns would inherit.
- **One act of the verb at any target, or the verb for the rest of the turn.**
  Rejected. Each lets the card under-state what the owner allowed.
- **A wall-clock expiry.** Rejected. A thread may wait on a lock for longer than
  any cap, and clause 6 already accepts authority spent while the owner is away.
- **A blocking CLI.** Rejected. A Bash call dies at 10 minutes, and an owner may
  answer hours later.
- **One CLI call, with the answer returned through an event wait.** Built
  first, then backed out. The agent had to end its turn while the card waited,
  and a card does not outlive its turn. Exempting approval cards from that rule
  touches the projection, the recovery guards, the follow-up supersede path and
  the frontend. It reopens the recovery gates this decision set out to leave
  alone.
