# 0170: The talker holds three tools: it asks, it answers what is waiting, and it hangs up

- **Status**: Accepted
- **Date**: 2026-08-31

Supersedes the one-tool clause of ADR 0164, which itself superseded ADR 0149's
tool-less clause. The rest of both stands, including the guarantee this one
keeps.

## Context

ADR 0164 gave the talker one tool, `delegate`, and said so twice: "The list is
one entry and stays one entry." It closed a real defect. It also assumed that
every future capability would be a capability, and it rejected a second tool on
exactly that reading.

Two things the talker cannot do turned up, and neither is a capability.

**It cannot pass on an answer.** A question card or a permission card parks the
thread's agent on a person. The caller says which one they want, and the talker
has nowhere to put it: `delegate` carries a reason, and a permission needs a
decision. So the caller has to reach for the screen, mid-call, to unblock work
they just asked for out loud.

**It cannot ring off.** A caller who says "that's all" has to find the button.
The talker holds no way to end what it is in.

ADR 0164's own argument settles who should decide both. Whether an utterance is
an answer, and which answer, is a judgment about what was said. Only the
participant that heard it is in time to make one.

## Decision

The talker holds exactly three tools, and the set is named inside the voice
module. `SessionOpening` still has no tool field, so nothing above the seam can
add a fourth.

1. **`delegate`**, unchanged from ADR 0164. One required reason.
2. **`answer`**, taking one required argument: a **choice id the engine
   issued**. It settles a question card or a permission card on the call's own
   thread.
3. **`hang_up`**, taking nothing. It ends the call.

**No matching, ever.** The engine reads what is open, issues an id per choice,
and reads those ids to the talker. An answer hands one back. So the engine never
compares a spoken word against a label, and the workspace's language cannot
break it. An id the engine did not issue is refused with a note saying so.

**The scopes a caller reaches are Allow once, Allow for this thread and Deny.**
Both Always-allow scopes stay on screen.

**Hanging up ends the CALL and never the work.** Four rules bound it. The
caller's words trigger it, never the talker's judgment. Silence is not those
words. Goodbye is said first, because the tool closes the line. And a turn in
flight keeps running.

## Rationale

**The guarantee survives, because it was never about the count.** ADR 0149's
real claim is that a wrong talker says a wrong sentence, while a wrong doer
sends an email. Measure each tool against that line rather than against the
list's length:

- `delegate` starts an ordinary doer turn and returns. Unchanged.
- `answer` presses a button the caller can already see, on their own thread. It
  reaches no workspace capability and can do nothing the caller could not do by
  tapping. What it CAN do is press the wrong one, which is the risk the issued
  id removes.
- `hang_up` closes a socket. It cancels nothing and writes one event.

None mutates the workspace. Every action still goes through the doer, with its
ordinary admission and its ordinary events.

**ADR 0164 rejected "two tools" on a case that is not this one.** Its rejected
alternative was a second tool "to say the answer is ready", and the reason was
that the existing `speak` path already did that. It bought nothing. These two
buy something that exists nowhere else: without `answer` the caller cannot
unblock the agent by voice at all, and without `hang_up` they cannot end the
call by voice at all.

**The trap ADR 0149 named is a curated CAPABILITY list, and this is not one.**
The trap was that every new engine tool becomes a judgment about whether voice
gets it. That judgment never arises here, because neither new tool is an engine
tool. The answer to "does voice get `send_email`?" is still no, unconditionally,
and it is still the doer's to run.

**An issued id is what makes answering safe.** The alternative is matching a
spoken word against a label, and it fails in every direction at once: the
workspace's language is a preference, labels are written by whichever agent
asked, and a wrong match presses a button the caller did not choose. Handing
back an id the engine minted removes the whole class. The engine cannot be wrong
about which choice was meant, because it never interprets.

**Both Always-allow scopes stay on screen, on asymmetry.** They widen what every
future agent session may do without asking. A caller saying "always" on a phone
usually means "stop asking me right now". Getting Allow-once wrong costs one
action; getting Always-allow wrong costs a standing grant nobody remembers
giving.

**The caller's words end a call, because the talker cannot see a reason to.**
Silence is the tempting signal and the wrong one: a caller who stops talking is
a caller thinking, and a dropped socket is already `Disconnected`. ADR 0149
already forbids the talker stating a fact it was not given, and "we are done" is
a fact only the caller has.

## Consequences

- **A caller can unblock the agent without touching the screen.** That is the
  point, and the two surfaces that read a card aloud now say so instead of
  sending them to the screen.
- The seam grows one member and loses one: `resolve_delegation` becomes
  `resolve_tool_call`, shared by every tool. Every `VoiceProvider`
  implementation owes one acknowledgement path rather than one per tool.
- `VoiceSessionEndReason` gains `AgentHangup`, told apart from the caller's
  `Hangup` for the reason `Disconnected` already is: the two read alike to the
  user and differently in the log.
- **A delegation is refused while the thread's doer is parked on a card.** The
  refusal states a fact rather than a policy: the agent is blocked inside the
  very call that asked, so there is no turn to start. Scoped to what actually
  parks the doer, which today is a question card or a permission card on the
  call's own thread.
- The doer is still never told a session is live. `purity_tests.rs` is
  unchanged, and an answer reaches the ordinary question and permission paths.
- **A card answered by voice is indistinguishable in the event log from one
  answered on screen, bar its actor.** Each lane grew one in-process resolver,
  and the consent endpoint calls it too. So that is structural rather than a
  property a test has to keep true.
- A fourth tool is a fourth ADR. The set is named in one module, and the seam
  carries no tool field. So growing it is a deliberate act rather than a
  parameter somebody passes.

## Alternatives considered

**Keep one tool and widen `delegate`.** Let the reason carry "the caller picked
the first option", and have the engine work out what that meant. Rejected: it is
matching a spoken sentence against a card, which is the whole class the issued
id removes. It would also route an answer through the doer, which is a turn
nobody needs.

**Keep one tool and let the engine detect an answer.** A heuristic over the
transcript while a card is open. Rejected as the wrong participant guessing,
which is ADR 0164's own reason for the tool existing at all. It is worse here
than for delegation, because a wrong guess presses a button rather than starting
a turn nobody hears.

**Answer with a label rather than an id.** Simpler for the model, and the label
is what the caller heard. Rejected: labels are written by whichever agent asked,
two can be near-identical, and this workspace speaks Norwegian while a label may
be English. A word list would be wrong by construction.

**Let a spoken answer carry free text as a second argument.** Rejected: a
question card is issued an extra choice meaning "they said something else", and
picking it sends the caller's transcript. One argument, and it mirrors the
screen, where the card's options sit beside a prompt textarea.

**Offer the Always-allow scopes too.** Rejected above, on the asymmetry between
what a wrong Allow-once costs and what a wrong Always-allow costs. Nothing is
taken away: the card keeps all five buttons on screen.

**Hang up on silence.** A timer, so a caller who wandered off is not billed for
an open socket. Rejected: it cannot tell a pause from a departure, and it would
cut off somebody thinking. A dropped socket already ends the call, and a real
one ends it correctly.

**Let the talker hang up when it judges the conversation over.** Rejected on
ADR 0149's honesty rule. Whether they are finished is a fact only the caller
has. A talker that ends a call on its own reading takes the phone away from
somebody mid-thought.

**Cancel the doer's work on hangup.** Tempting symmetry: the caller left, so
stop. Rejected because it is not what the button does either. Work asked for out
loud is still work asked for, and a caller who rings off expects to find the
answer waiting.
