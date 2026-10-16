# 0262: iOS autocorrect eats the tap on Send, so a per-device switch turns it off

- **Status**: Accepted, and amended the same day
- **Date**: 2026-09-23
- **Read first**: the amendment at the end. An unset switch now means on, on
  every client, and a device that keeps hitting the bug turns it off. The
  Decision below is the first version, which started iPhone and iPad with
  autocorrect off.

## Context

On an iPhone, in the installed PWA and a Safari tab alike, the composer's Send
or Submit sometimes did nothing while the keyboard was up. The reporter hit
it about twenty times from 2026-08-24. Twenty rounds of probes and recoveries
never named a cause, including [ADR 0183](0183-a-dead-composer-tap-runs-the-commit-face.md),
[ADR 0228](0228-composer-recovers-on-the-keyboard-close.md) and
[ADR 0245](0245-a-keyboard-close-is-seen-without-an-event.md).

The composer-press ledger of the 2026-09-23 episode settled four things:

1. **No touch reached the page while Send was dead.** The document saw no
   `touchstart` and no `click` between the last keystroke and the recovery. The
   probe records every touch on a watched face, so the taps never arrived.
2. **The page was alive.** Keystrokes landed, the scheduled checks ran, and a tap
   near the top of the screen did reach the page. The reporter made that tap on
   purpose, to test exactly this.
3. **A relayout changed nothing.** The recovery ran four times with the keyboard
   up, and Send stayed dead until the keyboard closed.
4. **The recovery was the reporter's.** Closing the keyboard freed Send, whether
   by the accessory bar's checkmark or by that tap on the empty pane.

WebKit forwards every touch its content view receives. `WKTouchEventsGestureRecognizer`
fails one only when `_shouldIgnoreTouchEvent` says so: a Live Text item, or a
touch that interrupts momentum scrolling. The momentum check would also have
dropped the tap near the top, which landed on a pane that does not scroll. So
iOS took the taps on Send before the web view saw them.

Three of the four screenshots attached to reports show iOS's autocorrect
underline on the last word, directly above Send or Submit. The fourth shows the
caret right after the last word. None of the four keyboards shows a prediction
bar, so Predictive Text was off.

Apple tracks a UIKit bug with this shape as FB13418977, "Autocorrect suggestions
sometimes eat touch events". While autocorrect holds a correction, a tap
elsewhere accepts it and never reaches the view it landed on. It began in
iOS 17.1 with Predictive Text off, and from iOS 18 it also appeared with it on.
Apple's developer support found no workaround.

The timeline fits. Autocorrect was off on every field from 2026-06-25 to
2026-07-29, as a side effect of the autofill fix, and no report came in that
window. It came back on prose fields on 2026-07-29, and the reports began four
weeks later.

## Decision

A per-device `autocorrect` preference decides whether prose fields autocorrect.
Unset, it is off on iPhone and iPad and on everywhere else. The switch lives
under Settings, System, Debugging, shown on iOS only. Spell-check underlines and
sentence capitals stay whatever it says.

An app's own text fields follow the same switch. The SDK stamps them inside the
app frame, by the rule the host uses, from one shared copy of it.

## Rationale

**The interception happens above the page, so only removing its trigger helps.**
No touch arrives, so no handler, gesture or relayout can answer it. Every
recovery the reporter found clears the autocorrect state: closing the keyboard,
or tapping into the text to move the caret.

**Off by default on iOS, because the failure is silent and the cost is small.**
The bug is in every current iOS keyboard, and a page cannot read the Predictive
Text setting that makes it worse. A dead Send is the worst failure the app can
have. The model reads typos well, so autocorrect buys little in a prompt box.

**A switch, because the reporter asked for one.** Anyone who wants autocorrect
back can turn it on, and the agent can flip it through the preference catalog.
It is device-scoped, since the bug belongs to one keyboard.

**Debugging is its home.** It works around a platform bug rather than setting a
taste. It shows on iOS only, because elsewhere it would change nothing visible.

**Why the relayout recoveries never worked.** They rewrite layout, and the state
that eats the tap lives in the keyboard. Their one apparent success, on
2026-09-20, landed in the same second as a keyboard close. Earlier rounds also
read the long silences as the wedge, when they were mostly typing time.

## Consequences

- On iPhone and iPad, text fields do not autocorrect by default. Typos stay
  underlined, and tapping one still offers replacements.
- The iOS default is a temporary measure in `docs/temporary-measures.md`. It
  ends when Apple fixes FB13418977, which a device check confirms.
- The composer probes and the relayout recoveries stay for a quiet period, then
  a follow-up deletes them. The registry rows carry that condition.
- An app's own text field is covered only when the app loads `sdk.js`. The
  host's stamp cannot reach an app frame's document, so the SDK stamps every
  text field there, prose or not. An app without the SDK is not covered.
- The diagnosis is inferred, not traced on a device. A dead-Send report from a
  device with autocorrect off would falsify it, and the quiet period watches
  for exactly that.

## Alternatives considered

- **Keep relayouting.** It cannot reach the keyboard's state. Four relayouts in
  the deciding episode changed nothing.
- **Widen the touch target, or run Send on a dead tap.** The touch never reaches
  the page, so there is nothing to widen and nothing to run.
  [ADR 0225](0225-composer-never-sends-by-itself.md) had already retired the
  second for sending a draft by accident.
- **Move Send away from the text.** The autocorrect state sits wherever the caret
  is, and the composer's buttons sit under the text by design. No placement is
  safe.
- **Return sends on iOS.** It adds a path the bug cannot block, but the button
  stays dead and Return stops making newlines.
- **The composer only.** Every prose field with a button under it has the same
  exposure, a trigger's intent above its Save for one. One rule is simpler.
- **On by default everywhere.** It keeps autocorrect for most users, whose
  Predictive Text is on. It lost because nothing tells an affected user that the
  switch exists.
- **Detect the wedge and suggest the switch.** The signals that mark it also mark
  a user who closes the keyboard to reread a draft before sending.

## Amendment, 2026-09-23: autocorrect starts on everywhere

The reporter reversed the first version the same day. An unset switch now
means on, on every client, iOS included. The switch stays under Debugging, and a stored
`false` still turns autocorrect off on that device alone.

Four things came back after it shipped:

- **Few people meet the bug.** A colleague uses Lucidos heavily on an iPhone
  and has never hit it, and no other user has reported it. The bug began with
  Predictive Text off, which is not how iOS ships, though reports since iOS 18
  include phones with it on.
- **The cost was not small.** Typing without autocorrect filled the reporter's
  own messages with typos. The Rationale above priced autocorrect low because
  the model reads typos well, but people read the transcript too.
- **The recovery is cheap, and users already know it.** Closing the keyboard
  frees Send. Some users would rather keep autocorrect and close the keyboard
  now and then.
- **The bug is not ours.** The reporter then met what looks like the same dead
  tap in another chat app on the same phone. The fault sits in the iOS
  keyboard, which every app shares. Other apps leave autocorrect on, and now
  Lucidos does too.

The alternative "On by default everywhere" lost because nothing tells an
affected user that the switch exists. That still holds, and it is the price of
this default. The Debugging explainer says what to do, and search finds the row
by "send button". The agent can also offer the switch to someone who keeps
hitting a dead Send.

The Consequences change with it:

- Text fields autocorrect by default on every client, iPhone and iPad included.
- `defaultAutocorrect` stays the one function that decides the default, and it
  now returns `true`. It lives in the text-entry module the host and the SDK
  share, so the composer and an app's fields go through the same function.
- The iOS default was a temporary measure. Its registry row is `removed`, and
  nothing waits on FB13418977 any more.
- The quiet period that retires the composer probes still counts from this ADR,
  whichever way the reporter's switch is set. With the switch on, a dead Send
  with no fresh correction above it still falsifies the diagnosis.
