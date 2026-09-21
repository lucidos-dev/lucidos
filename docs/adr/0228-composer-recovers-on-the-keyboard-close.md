# 0228: The composer relayouts on the keyboard close, and the probe names a silence

- **Status**: Accepted
- **Date**: 2026-09-20

## Context

The eighteenth report of the dead composer, and the first where the ledger holds
the wedge AND its recovery. The reconstruction is
[`docs/plans/2026-09-20-the-composer-recovers-when-the-keyboard-closes.md`](../plans/2026-09-20-the-composer-recovers-when-the-keyboard-closes.md).

Three readings from that episode, and the third is new.

- **Every layout-side reading said healthy.** Thirteen scheduled checks, and
  `unreachable: 0` and `covered: 0` on all of them. The reachability probe
  hit-tests a face at its own centre, and the face answers itself while the page
  receives nothing.
- **The wedge opened on a keyboard close.** The last working press was
  `keyboardActive: true, vvHeight: 476`. The window is `false, 852`.
- **A relayout ended it**, 36 seconds later, and only because the typing-driven
  trigger happened to come due. The tap one second after it was `served`.

One correction to the report went with it. The `served` line ending the window
carries `quiet.ms: 36345`, and the `untouched` line before it carries `1276`. So
two inputs reached the document inside the window and wrote no line at all. The
stray verdicts are gated on the keyboard being UP, and this wedge is
keyboard-down by definition.

[ADR 0183](0183-a-dead-composer-tap-runs-the-commit-face.md) gave the relayout
two triggers, both waiting for a touch.
[`docs/plans/2026-09-17-the-composer-recovers-without-being-touched.md`](../plans/2026-09-17-the-composer-recovers-without-being-touched.md)
added a third on the scheduled tick, gated on typing. None of the three can fire
at the moment the wedge starts.

## Decision

**The relayout fires on the keyboard-close transition, and it is a fix rather
than a diagnostic.** A `visualViewport` resize that restores the layout viewport
is the edge. It is driven from the app's own viewport handler, which already
owns `--app-height`, and it lives in `components/layout/keyboardCloseRelayout.ts`
so deleting the probe cannot delete it.

**The ledger gets `silent-since-keyboard`, a verdict for a wedge whose signature
is silence.** The scheduled check writes it once per close, when the composer
holds a live commit face and nothing has reached the document since. It carries
`sinceKeyboardMs` and `sinceInputMs`, so a reader can see an input that arrived
after the close and went unnamed.

**Five verdicts are retired, with the two apparatuses behind them.**
`commit-withheld` and `rescue-stood-down` went with the machinery that decided
whether to press Send, which [ADR 0225](0225-composer-never-sends-by-itself.md)
retired. `unreachable`, `repaired` and `repair-failed` went with the reachability
repair. `quiet.unreachable` goes with them, and `quiet.closes` joins the window.

**Nothing is pressed, on any path.** ADR 0225 is unchanged and unweakened.

## Rationale

The recovery should cost the user nothing, and a timer cannot deliver that. The
episode is the argument: the relayout worked, and the user still sat through 36
seconds of tapping to reach it. The close is a known, cheap, observable moment,
and it is when the wedge starts. A relayout there is one forced layout per
keyboard dismissal on a page nobody is stuck on.

The verdict exists because absence is not evidence. Twelve episodes died silent,
and this one shows why: the probe's own stray verdicts go quiet in exactly the
state the wedge lives in. A line that says "checks are running, the keyboard has
closed, and nothing has been touched since" turns the next episode into one
grep.

The cut is the other half, and it is what makes the fix affordable. The probe
was 2,127 lines defining eighteen verdicts, of which six appear in the kept
ledger. A general-purpose evidence collector earns its cost while the cause is
unknown. The evidence has now converged on one trigger and one recovery, so the
paths that serve neither are cost without a reading.

## Consequences

**The user should never see the wedge again.** If they do, the ledger says so
directly rather than by absence.

**The scoring rule from the 2026-09-17 plan is replaced.** That plan read "the
press after the nudge arrived with the keyboard DOWN" as proof the relayout ran
and did not help. This episode is exactly that shape and the relayout is what
freed it, so the old rule misreads it. Three readings replace it, and they are
exclusive:

| Ledger shape | Reading |
|---|---|
| A close, then an ordinary served press with `quiet.closes >= 1` | The wedge is gone |
| `silent-since-keyboard` with `nudged: true`, then a long `quiet.ms` | The relayout ran and did not clear it |
| `silent-since-keyboard` with `nudged: false` | `--app-height` was not ours to write |

The second reading is what would retire the bounce and leave the transform
option next.

**An earlier ledger still reads.** Every surviving field keeps its exact
meaning, `quiet.nudges` and `nudgesSinceKeystroke` included: they stay the
typing-driven recovery's score, and the transition relayout is counted apart as
`quiet.closes`. The five retired verdicts are named here, so an old line is
still explicable.

**The probe is evidence plus one recovery.** The typing-driven trigger stays,
because round 14's wedge began with the keyboard UP and had no close in front of
it. Its removal condition is correspondingly shorter: `bounceHeight` and the
relayout have already moved to a permanent home.

**A cover no longer stands anything down that could act.** The reachability
repair was the only thing a cover refused, and the `covered` line survives it.

## Alternatives considered

**Widen the existing throttle.** Make the scheduled tick faster, or loosen
`shouldNudgeUntouched`. Rejected outright by the user, and rightly: it shortens
the silence rather than removing it, and it pays for that on every page in every
session. The transition costs nothing on a healthy page.

**Relayout on the keyboard OPEN as well.** Symmetrical and cheap. Rejected on
the evidence: every episode with a keyboard reading opens on a close, and a
layout per focus buys no reading anybody has asked for.

**Drop the keyboard-up gate on the stray verdicts**, so the two unnamed inputs
in the episode would have written lines. Rejected on noise. The gate is what
keeps a line unusual. Dropping it writes one for every scroll and every tap
anywhere in the app, four a second. The silence line carries both anchors
instead, which makes the same inference from one line.

**Keep the retired commit's machinery for the evidence.** ADR 0225 says a run of
`commit-withheld` lines at a `reachPx` of zero is what would justify bringing the
commit back. Rejected on two counts. That reading has appeared in no ledger, and
the machinery runs on every touch to produce it. Bringing the commit back is an
ADR in its own right, and it would arrive with its own instrument.

**Keep the reachability repair.** It is the only check immune to a coordinate
space out of step with layout, which is a genuine property. Rejected because it
has answered healthy through every episode it was built for, including all
thirteen checks of this one. The report says it structurally cannot see this
state, and the silence verdict now covers what it was reaching for.

**Retire the whole probe.** The cause is still not named, and no emulator
reproduces the wedge, so the ledger is the only instrument there is. The cut
takes the paths that serve no reading and keeps the ones that do.
