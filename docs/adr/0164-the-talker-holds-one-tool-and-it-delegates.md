# 0164: The talker holds one delegation tool and decides when the doer runs

- **Status**: Accepted
- **Date**: 2026-08-30

Supersedes the tool-less clause of ADR 0149. The rest of that ADR stands,
including the guarantee this one keeps.

## Context

ADR 0149 opened the talker with an empty tool list, and recorded the cost in
its own consequences: "Every spoken turn starts a doer turn, because the engine
decides and the talker cannot."

The first real call showed what that cost is. The caller said "hei". The talker
answered from the resident block, and the engine spoke the doer's answer on top
of that. So they heard "Hei" and then "Hei" again in another language. Every
turn produced two complete answers, and a turn the talker could handle alone
was the worst case rather than the cheap one.

Nothing on the engine's side can tell the two apart. Whether an utterance needs
tools is a judgment about what was said, and the only participant that has
heard it in time is the talker.

## Decision

The talker holds exactly one tool, `delegate`, taking one required argument: a
short reason. It decides whether an utterance needs the doer.

**The tool is an ask, not a wake.** The talker is never told whether a doer
turn is running, and is never asked to find out. It delegates every request
needing the doer, and single-flight admission decides whether that starts a
turn or joins one. So an utterance during a running round reaches that round,
and the talker's instruction names no state it could read wrong.

The list is one entry and stays one entry. `SessionOpening` still has no tool
field, so the single tool is named inside the voice module and nothing above
the seam can add a second.

## Rationale

**The guarantee survives, because it was never about the count.** ADR 0149's
real claim is that a wrong talker says a wrong sentence, while a wrong doer
sends an email. One tool that starts a turn and returns is still on the
harmless side of that line: it mutates nothing, reads nothing, and reaches no
workspace capability. Every action still goes through the doer, with its
ordinary admission and its ordinary events.

**An empty list was the wrong shape for the requirement.** What ADR 0149 wanted
was no *capability*, and it bought that with no *tools*, which was cheaper to
state and easy to enforce. The two came apart the moment a delegation was
needed, because delegation is a tool call that grants nothing.

**Only the talker is in time.** The engine sees a finished transcript and no
more. The talker has heard the utterance, holds the resident block, and knows
whether it can answer. Asking it is the only place the decision can be made
before the caller hears anything.

**The tool description biases hard toward calling.** The resident block is a
snapshot taken when the call opened, and nothing corrects it once the doer
stops running every turn. Under-calling therefore answers confidently from
stale data, which is the failure a listener cannot detect. Over-calling costs
one turn nobody hears. The two are not symmetric, and the wording says so.

**The wake fires on the tool-call frame, not the turn's end.** A function call
lands while the talker is still speaking. Waiting for the turn to finish would
put a whole spoken reply in front of every real question. That is the dead air
voice has to avoid first.

## Consequences

- A turn the talker can answer costs one model and one answer. That is the
  defect this closes, and the open question ADR 0149 left behind.
- Nothing spoken can still act. The one tool starts an ordinary doer turn and
  returns, so every action keeps going through the doer.
- **Nothing corrects a stale resident block.** The doer no longer runs on every
  turn, so a talker that under-calls answers from a snapshot with nothing
  behind it to notice. The instruction leans against that, and only a real call
  shows whether leaning is enough.
- An utterance the talker handles alone needs its own row.
  `MessageReceived` is a Start event, so using it for a turn that will not run
  would leave the thread claiming one. `SpokenMessageReceived` is what carries
  it, and `WorkDelegated` records the ask beside the turn it started.
- A delegation is written down and names the talker, so the thread holds all
  three participants: what the caller said, what the talker asked for, and what
  the doer did (ADR 0150).
- The seam grows two members, `DelegationRequested` and `resolve_delegation`.
  Every `VoiceProvider` implementation owes both, which is the cost of the tool
  not being a `SessionOpening` field.
- `call.rs` now buffers a finished utterance until its fate is known. That is
  new state on the call, and it is why a call's end flushes whatever is held,
  for every end reason.
- The doer is still never told a session is live. `purity_tests.rs` is
  unchanged, and the delegation reaches it as an ordinary message.

## Alternatives considered

**Keep the empty tool list and gate in the engine.** A heuristic over the
transcript: length, question words, whether the resident block mentions the
topic. Rejected as the wrong participant guessing. The engine would rebuild a
judgment the talker already made, from less than the talker had. Every wrong
guess is then a spoken answer the caller cannot audit.

**Ask the talker, but at the end of its turn.** Read the decision off
`TalkerTurnEnded` instead of the tool call, keeping the seam smaller. Rejected
on latency: it puts a whole spoken reply between the question and the work,
on every real question. Delegation has to be free or the talker learns to avoid
it.

**Two tools, one to delegate and one to say the answer is ready.** Rejected as
a second registry in miniature. Every new engine capability would become a
judgment about whether voice gets it, which is the trap ADR 0149 named. The
second tool also buys nothing the existing `speak` path does not.

**Tell the talker whether a turn is running, so it can skip a redundant ask.**
Rejected twice over. It would need per-session state in the cached prefix,
which the instructions cannot carry, and it asks the talker to model something
it cannot observe. A caller who speaks mid-turn needs that second ask: it is
what carries the utterance into the round already running.

**A `tool_choice` of `required`, so the talker always delegates.** Rejected
because it is the old behaviour spelled differently. Every turn would wake the
doer, and the caller would hear two answers again.
