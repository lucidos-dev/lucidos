# 0183: A dead composer tap runs the commit face itself, then the bounce

- **Status**: Accepted
- **Date**: 2026-09-12

## Context

Twelve reports of the same thing, in the reporter's own words: while the
keyboard is up the composer cannot be tapped **anywhere**, and it comes back
only when the keyboard is dismissed and reopened. It is not about which pixel
the finger lands on. Every round that treated it as aim has been wrong, and
this ADR exists partly to stop the next round doing it again.

The probe cannot see the cause, and the reason is structural. Every reading it
takes comes from the layout side: a rect, a hit test at that rect's centre, a
viewport height. Those agree with each other during an episode, because they
are all downstream of the same layout. The disagreement the user is describing
is between layout and the glass.

What the ledger does show, from the twelfth episode:

```
19:19:59  missed  the row  morph=send  under=nothing  at=div.prompt-actions-row  kbd=on
19:20:00  served  Send message
```

The face was live and reachable at its own centre. The press reached the row
and no face. Most taps in an episode write no line at all, because a press the
probe cannot attribute to the composer returns before it records anything.

## Decision

A stationary tap that lands on the composer and reaches nothing gets two
answers, in this order.

**It runs Send.** The app already knows enough: a touch reached the document,
its coordinates were inside the composer row, no button claimed it, and the
morph is live. So the press is activated directly, and the message goes.

**Then it relayouts the shell**, a keyboard's span away and straight back, which
is the user's own recovery without the keyboard. Send answers the tap just made;
the relayout is for the next one.

Four bounds on the Send half, each a state where the intent is not certain:

- A COMMIT face only, so a dropped tap can never stop a running turn.
- The keyboard must be UP, which is the state every report describes.
- Nothing may have claimed the gesture. The rescue waits out the click grace
  window and stands down on any click, and on a later press that reaches a real
  face.
- The row is re-read at the moment of firing. A second tap that got through in
  the meantime has already moved it off a commit face, so this does nothing.

**A commit face is the button that sends what the user typed.** Two qualify,
and the row renders exactly one of them: the Send morph in `send` mode, and the
answer Submit while a question is pending. Nothing else. A destructive face
must never run on a tap nobody saw land, and Apply wears the same confirm green
while merging a change nobody approved.

The first draft of this decision said SEND mode only, and the thirteenth
episode found that out. It was in answer mode, where `computeMorphMode` renders
no morph at all, so the rescue opened with a test it could never pass. The
user's typed answer sat unsent while the app relaid out the shell and said
nothing.

That bound was never a decision about Submit. It was drawn to exclude Cancel,
and Submit commits typed text exactly as Send does.

The bounce goes DOWN, though the keyboard's goes up. Growing the shell shrinks
every scroller in it. The browser clamps their scroll offsets at that layout,
and restoring the height does not restore the offsets. A transcript at the live
edge would jump most of a screen on every dead tap. Shrinking only makes room,
so nothing is clamped and the restore is exact.

## Rationale

The recovery is the one fact twelve reports agree on. We do not know what
WebKit has got stuck, and a fix aimed at a mechanism we cannot name would be
the thirteenth guess. A fix aimed at the RECOVERY needs no mechanism. Whatever
the keyboard bounce clears, the same relayout clears, because the bounce works
by rewriting that height and nothing else we own.

Doing it on the first dead tap is the whole value. The user currently taps ten
times, gives up, dismisses the keyboard by hand and taps again. The app can
spend that recovery on tap one.

The BOUNCE is silent, because nothing can score it. The repair the reachability
check drives knows it worked: the face was not answering at its centre, and
afterwards it is. Here the face answers throughout, so a toast would claim a fix
on every stray tap on empty row space.

The SEND is not silent. The app took an action the user did not see land, so it
says so. That also keeps the bug visible: without the toast the wedge would stop
being reported the moment it stopped being felt.

## Consequences

A dead composer costs one tap. The message goes on that tap whether or not the
relayout clears the wedge, which is the point: the Send half needs no theory
about the cause at all. The log carries an `activated` line for it, and a
`repaired` or `repair-failed` line for the bounce behind it.

The bounce is two style writes and two forced layouts in one task, so the user
sees nothing and no scroll position moves. It fires only for a stationary
press, so a swipe or a scroll that begins on the composer is untouched.

A row-missed press is now ruled at its lift like every other press, and carries
the travel it actually measured. Until now it asserted `movedPx: 0` at
touchdown, so a swipe beginning on the row and a tap dying there wrote the same
line.

Every press line also carries `screenOff`, the difference between the touch's
screen and client coordinates. No other reading here comes from outside the
layout. So the next episode can state whether the page hit-tested where the
finger actually was.

A touch that reaches the page while the keyboard is up and does NOT reach the
composer now writes a line too, throttled. That closes the blind spot every
round has died in: "no line" has meant both "iOS delivered no touch" and "iOS
delivered it somewhere the composer is not". The two have different fixes and
no shared one, and until now nothing in the app could tell them apart.

**A silence has two more meanings, and both are ours rather than the
platform's.** A press the probe declines under the app's own cover writes
`covered`, and a touchless click that reaches no composer face writes
`stray-click`. Every line also counts the scheduled checks a cover stood down,
which is the only reading available across a stretch nobody touched. So the
next blank ledger names one state instead of three.

## What the platform research says

Four sources, and together they place the fault outside our code.

- [WebKit 237851](https://bugs.webkit.org/show_bug.cgi?id=237851) is
  standalone-web-app specific: with the keyboard open, `visualViewport.offsetTop`
  is reported as `0` when it is really several hundred px. Open since 2022,
  assignee nobody. Our ledger reads `vvOffsetTop: 0` in every line.
- The [iOS 26 viewport thread](https://developer.apple.com/forums/thread/800125)
  carries the consequence in a developer's own words: "all the background
  elements (hit areas) become unaligned with the UI buttons". Apple DTS has it
  open with engineering.
- The WKWebView form of it is older and better described. The web view scrolls
  itself to reveal the focused input and does not restore the offset. **Every
  touch is then displaced by that amount, while the page still reads `scrollY`
  as zero.** Rotating the device clears it for one round
  ([rdar://44655885](https://openradar.appspot.com/44655885),
  [WebKit 192564](https://bugs.webkit.org/show_bug.cgi?id=192564)).
- The [standalone-PWA write-up](https://dev.to/cederhook/fixing-the-ios-standalone-pwa-keyboard-bug-that-shrinks-your-viewport-for-good-63d)
  adds the one that closes an option: **`interactive-widget` is ignored in
  standalone mode.** It helps in mobile Safari and nowhere else.

That last point retires the viewport-contract candidate this ADR used to leave
open. The displacement reading explains the rest. Paint and rect agree, and the
reachability check passes, yet the finger reaches the wrong element. The only
wrong number is the one attached to the touch.

## Alternatives considered

**Make the tap target bigger.** Shipped, then reverted within the hour. The
reporter's answer was immediate and unambiguous: it is dead regardless of where
the finger lands. A 29px circle against the 44px minimum is a real finding
about the composer, and it is not this bug.

**Toast on a dead tap.** Five episodes produced five reports of a toast nobody
could act on. A recovery the user does not have to read beats a message.

**Wait for one more episode with better instruments.** That is what the last
four rounds did. The reading is still worth adding, and it is no longer the
deliverable.

**Change the keyboard's viewport contract** (`interactive-widget`). Closed by
the research above: the attribute is ignored in standalone mode, which is how
this app is used. It would have been a no-op dressed as a fix.

**Position the composer by transform instead of shrinking the shell.** Still
open, and still unverified. The argument for it is that the community's most
stable workaround is transform-based positioning. The argument against is that a
composited transform is a classic way to split the paint layer from the
hit-test layer. That is the very failure being chased. It could make this
worse, it changes every mobile surface, and it cannot be checked without the
device.

**Return-to-send on mobile.** REJECTED BY THE USER, twice. Do not propose it
again. A key press bypasses hit testing entirely, so it is technically the one
channel proven to survive the wedge, and that is not enough: the return key is
a newline on a phone and the user has said no. This paragraph exists so the
next session does not rediscover the idea and spend the user's patience on it.
