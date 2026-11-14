# 0664: One permission model, and the automation pause

- **Status**: Accepted
- **Date**: 2026-11-12
- **Amends**: ADR 0168 (clause 5), ADR 0362 (Always allow on screen only), ADR 0387 (owner approval), ADR 0416 (proposal hold), ADR 0524 (plan approval)
- **Amended by**: [0671: The workspace broker](0671-workspace-broker.md) (seven workspace actions, a workspace classification batch item); [0688: Every Allow once card is a permission request](0688-every-allow-once-card-is-a-permission-request.md) (every Allow once card joins the model, with per-option breadth)

## Context

A coding-agent thread was told, through a relaying agent, to checkpoint and
pause. It stopped and ended its turn. At that idle the proposal hold injected
its `/harden` nudge, and the agent obeyed. The pause lived only in chat text, so
the engine could not see it.

Fixing that one path exposed a wider gap. Lucidos already had four lists of the
same idea: owner approvals, the command, coding-agent and MCP permission cards,
the thread buttons, and the cross-thread reach verbs. Each had its own words,
its own storage and its own rules for who may answer. Voice could answer some of
them, chat none.

## Decision

**One permission model.**

- An **action** is one entry of the `Action` enum. Buttons, tool calls, CLI
  commands, engine steps and voice name the same entries.
- A **permission request** is engine-worded and offers typed options. Every input
  picks one option: the owner's tap or the owner's words. Another thread relays
  an answer only through a batch the owner confirms.
- A **permission** is what the picked option grants. It names an action and its
  breadth. Its lifetime is once, this thread, until the plan lands, or standing.
  Its holder is a thread, a trigger or the workspace.

**Words grant only with a judgment.** The message must come from the owner's
device, after the request. A System One judgment,
falling back to the chat model, must confirm the words choose that option. No
model, or an unsure judgment, grants nothing. A tap always works.

**Spoken words count for everything.** They must come from the owner's own
transcript, on a call ticket their attested device minted (ADR 0598). The voice
agent's summary never counts. A wide grant is read back first, and the
transcript must name it. This covers the two widest Always-allow choices too,
which ADR 0362 had kept on screen only.

**A batch is one request carrying several exact items**: question cards,
permission cards, plan approvals and trigger grants, gathered from the
sub-threads under a parent. The engine words it and stores its hash. One confirm
spends it once. Each item is checked again when it runs, and any change fails
that item closed. A thread answers for the owner only through a batch in its
own thread, never by quoting the owner.

**Four rules keep card answers from closing a circle.** Only the owner's input
picks an option. A batch never holds a batch, and the engine words every batch.
Card answers and plan approvals are never granted beyond one exact item. No
thread raises a batch holding its own card.

**A plan approval binds to the plan text** (amends ADR 0524). A spent batch
item counts as the user's pick for that exact text, and a re-mark needs a new
yes.

**A trigger grant binds to a definition digest**, a hash of `run`, `on`, cron,
MCP servers and app. Secret reads are never grantable.

**Clause 5 no longer covers card answers** (amends ADR 0168). Answering another
thread's card, in the caller's subtree or not, needs a matching permission or
batch item. Other owner buttons keep clause 5.

**The pause family shares one root, prefixed by what is paused.**

- **Thread pause**: the restart status. The whole thread is paused until the
  engine resumes it.
- **Agent pause**: typed messages stop waking the agent (ADR 0649).
- **Automation pause**: the engine's standing permission for automatic steps is
  paused on one coding-agent thread. Ended by **unpause**.

While an automation pause stands, each automatic step raises a permission
request instead of running: the harden nudge, the plan nudge, the context
handoff and reading waiting reports. Waiting reports share one growing request.

## Rationale

- **Prompt-first** (`docs/philosophy.md`, rule 2): anything a person can tap must
  be doable by asking. A single action list makes that a property of the type
  system, not of each surface.
- **The engine words every option.** An agent cannot invent a scope or a lifetime
  no option offers, so ADR 0651 still holds: an ordinary answer grants nothing.
- **A judgment, not a parser, reads words.** Exact phrases fail on voice and on
  paraphrase. A fallback that guesses would grant on a misreading, so the
  fallback is to grant nothing and leave the tap.
- **The pause restricts the engine, not the people.** Messages still arrive and
  the running turn finishes, because stopping either loses work. Only the
  steps the engine would start on its own ask first.
- **Auto-resume after a thread pause continues.** It finishes a turn the owner
  already started, so pausing it would leave work half done.

## Consequences

- Owner approvals and the three permission cards become permission requests.
  Their grant files and session allows stay where they are until a later plan
  moves them.
- A batch answers a tool-permission card through the card's own resolver, so
  it stores what the button stores. It picks Allow once, this thread or Deny.
  An Always allow stays the owner's own tap, since a batch item is never a
  standing power.
- Owner approval cards join batches. The engine words each line from the typed
  approval, never from the agent's question, and the answer carries the
  confirming device.
- A card parking an unverified caller's act stays out of batches. Its Allow
  replays the request through the HTTP router, which a batch running inside
  the engine cannot reach.
- An open mic cannot tell the owner apart from someone else in the room, a TV
  or a recording. The owner accepted that risk for voice on every grant. The
  read-back, the attested ticket, the receipt and revoking are the guards.
- A batch holds what exists when the owner confirms. Pre-approving an item that
  does not exist yet would be consent to something nobody was shown.
- An automation pause survives restart, because it is events folded at read
  time, like the agent pause.
- Unhardened work on a paused thread stays unproposed and says so. The
  open request makes the thread Blocked.
- A coding-agent start from a caller Lucidos cannot verify waits on its card
  the same way. The gate writes no failure, and Don't allow closes the start
  with `ResponseCanceled`, cause `refused`.
- "Owner hold" and "act" were names for these concepts during design. Neither
  shipped.

## Alternatives considered

- **An owner hold with fixed release rules.** The first plan. It hard-coded which
  acts released the hold and withheld every engine turn, including auto-resumes
  and child reports. Rejected: it lost work on restart, and it was a fifth list
  of the same idea.
- **Approvals bound to a turn or a branch head.** Rejected: what matters is the
  exact thing allowed, not when it was asked. The lifetime belongs in the option.
- **A free-form action string.** Rejected: drift between surfaces is the problem
  being solved. An enum generated to TypeScript makes a missing surface a
  compile error. Each gated handler names its action where it checks authority,
  so a manifest tag would only copy that value.
- **Grant on words without a judgment model.** Rejected: a keyword match grants
  on "don't continue". Taps stay as the floor.
- **Quoted relays.** A thread cites the owner's message and a judgment checks
  it means the act. Rejected: a quote proves some words were said, not that
  they meant this act. A batch shows the owner the exact act instead.
- **Keep the widest Always allow on screen only.** Offered as the cautious
  option. The owner chose voice for everything, with the read-back as the guard.
- **Pause and resume as the words.** Rejected: "resume" already means a process
  carrying on (`--resume`, auto-resume). Unpause has one meaning.
