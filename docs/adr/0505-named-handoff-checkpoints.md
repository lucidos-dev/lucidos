# 0505: Handoff checkpoints are names that flows pass: amends 0499 and 0503

- **Status**: Accepted
- **Date**: 2026-10-17
- **Amends**: [0499](0499-successor-offer-follows-effort-and-checkpoints.md), [0503](0503-build-chain.md)
- **Amended by**: [0507: A successor inherits the consent that started it](0507-successor-inherits-handoff-consent.md)

## Context

Two threads showed the gaps in the checkpoints of ADR 0503.

- A re-design dialogue grew from 57k to 416k tokens. Its plan marker was still
  approved from its predecessor, so it never ran `lucidos planned mark`, and no
  card offered a new thread. The user had to type the handoff by hand.
- A thread entered `/harden` at 272k. The estimate read one median over every
  harden run, about 21 calls, and said a handoff does not pay. Runs that enter
  at 250k or more take a median of 38 calls (27 runs), against 20 below. At 38
  the handoff pays about 2 to 1.

The checkpoints were also hardcoded. `pre-harden` and `phase-commit` were
engine enum variants, with harden-only SQL behind the first. An external
repository could name only an offered checkpoint. Lucidos did not use its own
flows the way a user's repository would.

Plan: `docs/plans/2026-10-17-named-handoff-checkpoints-and-default-flows.md`.

## Decision

- **Lucidos owns primitives, repositories own flows.** The engine ships named
  *handoff checkpoints* and the *handoff estimate*. A repository opts in by
  calling a checkpoint from its plan skill, review flow or agent instructions.
  There are no toggles, and a repository with no checkpoint gets nothing.
- **The engine names no checkpoint.** `pre-harden`, `phase-commit` and
  `plan-approval` are names that flows pass. Any name of lowercase letters,
  digits and hyphens is valid.
- **One command for both kinds.** `lucidos successor offer --at <name>` runs an
  automatic checkpoint, which asks nothing. With `--ask` it runs an offered one,
  which prints a question card. `--progress <done>/<total>` is optional on both.
- **The estimate decides the offered card too.** Plan approval is `--at
  plan-approval --ask`. Its card recommends a new thread when the estimate pays,
  in place of the fixed `HANDOFF_OFFER_TOKENS` line. The cheaper pick (ADR 0497)
  and the raise below `high` (ADR 0499) stay.
- **The unit of work is the measure.** Units left are `total - done`, else one.
  Calls left are units left times calls per unit.
- **Calls per unit come from history.** Every checkpoint run is recorded as a
  `HandoffCheckpointReached` event. A sample is the main-session calls from that
  event to the first *clean turn end*, divided by the units left then.
- **The end point is the clean turn end.** It is a `CodingAgentIdled` with no
  open question, no event wait and nothing unread. If the thread hands off
  before it, the count follows into the successor, up to its own clean turn
  end.
- **Samples pool on repository, name, model and effort.** The median first reads
  samples entered at half to double this context. Under 3, it widens to all
  sizes. Under 3 again, one default constant applies.
- **A re-plan runs the approval checkpoint again**, and `planned mark --plan`
  re-proposes an approved marker, so the edit gate closes until the new
  approval.
- **The default flows ship as system knowhow.** `coding-agent-flows` explains
  how to wire a checkpoint. Two default flows, an implementation plan and
  hardening, carry them already wired. An agent copies one into a repository
  only when the user asks, and the repository then owns its copy.
- **Lucidos dogfoods the defaults.** Its `implementation-plan` skill and
  `harden.md` hold each default verbatim inside marked blocks.
  `./scripts/check-default-flows.sh` fails in `/harden` when a block and its
  knowhow differ.
- **The main check's fix thread runs an automatic checkpoint**, `root-cause`,
  once it knows the cause. It used to print the offered card, which parks an
  unattended thread on a question.

## Rationale

**A flow knows its own units; the engine does not.** A plan knows how many
phases it has, and a review flow knows it is one unit. The engine needs only a
name to pool history on and a count of what is left.

**History beats a constant, and the size band beats one median.** The calls a
unit takes vary by repository, by model and effort, and by how much context the
agent already carries. A median over every run hid the 38-against-20 split
above. Pooling on the strict key keeps one repository's habits out of
another's estimate.

**The clean turn end works for every repository.** An external-repo thread never
proposes a change, so the proposal could not serve as the end point. A turn that
ends with nothing pending is where the work of a checkpoint is done.

**Following the handoff keeps the sample whole.** Without it, a thread that
hands off would record only the calls before the handoff, and the estimate
would shrink each time it paid.

## Consequences

- An external repository can name automatic checkpoints, and the build chain's
  consent (ADR 0503) holds for any name whose estimate pays.
- Past runs carry no checkpoint events, so every pool starts on the default
  constant until it has three samples.
- The successor strip keeps `HANDOFF_OFFER_TOKENS` and `HANDOFF_NUDGE_TOKENS`.
  Only the plan card stops reading the offer line.
- A long design dialogue before any plan still has no checkpoint. The user can
  say "handoff this session", and the strip offers it at a clean idle.

## Alternatives considered

- **Per-repository toggles.** A switch per checkpoint in the repository's
  settings. It adds a second place to look, and a repository that calls a
  checkpoint has already said it wants one.
- **A repository config file.** A file listing checkpoints and their units. The
  flow already says where each checkpoint runs, so the file would restate it
  and drift from it.
- **A plugin per flow.** It packages what one paragraph of knowhow carries, and
  hides the flow from the repository that should own it.
- **A fitted line of calls against context size.** It needs more samples than a
  pool on the strict key will hold for weeks. The size band captures the same
  bias with the samples there are.
- **The proposal as the end point.** It fits Lucidos threads only. An
  external-repo thread proposes nothing, so its samples would never end.
