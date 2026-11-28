# 0803: One open card per thread: a single definition shared by engine and app, and an open card blocks its thread

- **Status**: Accepted
- **Date**: 2026-11-23
- **Amends**: ADR 0755 (when a card is open)

## Context

Five readers answered "is this card open?", and each had its own rule:

| Reader | Its rule for a card Lucidos raised |
|---|---|
| Card slot | Closed once the agent took any step |
| Typed-message hold | Open until its own answer |
| Step dedup | Closed once the turn moved on |
| Unpause sweep | Closed once overtaken |
| The app | Dead when the turn it landed in ended |

So one card could be dead on screen, open in the engine, and holding the
owner's messages. A queued engine card posted mid-turn, the turn ended, and
the thread deadlocked: the owner could not tap the card, and every message
waited behind it. The card slot also let a second card post beside one the
hold still counted open.

## Decision

The engine holds one definition of an open card,
`question_park_sql::open_posted_card_sql`, read through the typed `OpenCard`
reader. The app draws the same rule.

- **A card Lucidos raised** (a permission request no agent prepared) is open
  until its own answer. A retirement is an answer of kind `Superseded`. A
  turn moving on never closes it.
- **An agent's card** is open until its answer, its own tool result, or its
  turn's end, as before.
- **An agent permission prompt** is open until it is resolved.
- **The oldest open card is the thread's open card.** Several exist only on a
  thread from before this decision. The others wait behind it: Validate
  refuses an answer to one with `card_waits_behind_older`, and the app draws it
  disabled with "Waits for the earlier card". A Cancel is never refused.
- **The app imports the overtaken list** as the generated
  `QUESTION_OVERTAKEN_EVENT_TYPES`, never a hand copy.
- **A typed message held behind a permission card** says so on its row:
  "Queued until you answer the open card".

### An open card blocks its thread

- **A card Lucidos raises while a turn runs queues** as a *queued card*, and
  the card queue posts it when the turn ends. A card a turn holds for posts at
  once: the call polls for it (`run-confinement-tests`), or the run parks on it
  with no terminal (a remote start, a trigger run's grant). The raiser says
  which, with `raise_turn_holding_request` and the request's `holds_a_turn`.
- **No turn starts on a thread while a card is open**, except from that card's
  answer. One admission, `step_hold`, holds every engine step: an event-wait
  delivery, a turn-end nudge, a context-ceiling re-entry, a report read, a boot
  resume, Continue and the successor session. A child report to a held
  coding-agent parent is filed. An agent's message waits in the Thread Queue
  executor. The owner's typed message waits behind a permission card as
  before, and supersedes the agent's own question as before (ADR 0082). The
  card's first close hands the held wakes over (`card_hold`).
- **While the agent's own question is open, its other tool calls are
  refused** with `OPEN_QUESTION_TOOL_REFUSAL`:
  - Chat agent: the tool dispatch. Its question runs inline, so this is the
    floor.
  - Claude Code: a `PreToolUse` hook on every tool, `lucidos
    cc-question-guard`. It reads the session transcript: Claude Code writes each
    block before it runs that block's tool, so a sibling streamed after the
    question always finds it there. A sibling streamed before the question ran
    before it was asked. The hook costs a process start, about 11 ms, against
    about 90 ms for a hook that calls the engine.
  - Codex: the approval bridge, for commands and file changes only. **Not
    covered:** a Codex read, a sandboxed command that needs no approval, and an
    MCP call. Codex sends nothing else to the engine before it runs, so no
    authority exists to stop them.
- **Retirement.** A card nobody waits for closes as `Superseded`
  (`card_retirement`): every card on a thread when it is archived, and a
  turn-holding card when the turn that held it ends.

The *question park* stays a separate predicate. It answers "is a turn parked on
a question?" for the restart preserve guard, and a card Lucidos raised after
its turn ended parks no turn.

## Rationale

- **The card's own answer is the only fact that cannot go stale.** A turn's
  end says something about the agent, not about a card the agent never
  waited on.
- **Retiring stays possible.** A producer that no longer wants its card
  answers it `Superseded`, which the rule already counts as closed.
- **Oldest first keeps one card in front of the owner** without rewriting
  stored events. No migration touches the event log.
- **Generated, not mirrored.** The hand mirror of the overtaken list was the
  third copy of one rule. Codegen makes drift a build failure.

## Consequences

- An old orphaned engine card is now open in every reader, and live on screen.
  New cards queue behind it until the owner answers or cancels it. This makes
  stuck cards answerable instead of silently blocking.
- The voice talker reads the open card, oldest first, rather than the newest.
- An agent's message waits in the executor while a card is open, so it holds
  its Thread Queue slot like a held typed message does.
- A card raised from a turn that never ends, with no `holds_a_turn`, would
  never post. Every such raiser today sets it; a new one must.

## Alternatives considered

- **Close an engine card at its turn's end.** The step would be asked again
  next turn, and a permission the owner never saw would read as refused.
- **Keep the readers and fix only the app.** The card slot would still let a
  second card post beside an open one.
- **Retire every legacy card at boot.** That loses asks the owner never
  answered. Retiring needs a reason, such as the work being gone.
