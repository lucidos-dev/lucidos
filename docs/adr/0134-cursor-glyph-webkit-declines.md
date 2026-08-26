# 0134: Why the macOS cursor is always an arrow: WebKit declines, and mirroring cannot help

- **Status**: Accepted
- **Date**: 2026-08-25

Supersedes [ADR 0129](0129-native-cursor-mirroring.md).

## Context

The packaged macOS app showed the plain arrow everywhere: over pane dividers,
over buttons, and over text fields. ADR 0129 diagnosed this as a race between
two writers and shipped *native cursor mirroring* in v0.30.2. The frontend read
the computed CSS `cursor` on `pointerover` and pushed a matching `CursorIcon`
to `Window::set_cursor_icon` over IPC.

It changed nothing. The user reported the identical symptom after the release,
possibly slightly worse.

This ADR records what the follow-up investigation established. Most of ADR
0129's technical premises turned out to be false, so the record has to say so.
The revert itself is
[`docs/plans/2026-08-25-revert-native-cursor-mirroring.md`](../plans/2026-08-25-revert-native-cursor-mirroring.md).

## Decision

Remove the mirroring in full. Record WebKit's own `setCursor` guards as the
real mechanism, and leave the symptom unfixed rather than shipping a second
mechanism that cannot reach it.

## Rationale

### tao installs no cursor rect, so there was never a second writer

ADR 0129's premise was "two writers, one cursor": tao lays an arrow cursor rect
over the whole content view, and AppKit re-asserts it as the pointer moves,
stomping WebKit's glyph. Four lines of the tao 0.35.3 we build against refute
it:

| Site | What it says |
|---|---|
| `util/cursor.rs:23-25` | `enum Cursor` derives `Default`, with `#[default] Default` |
| `util/cursor.rs:35` | `CursorIcon::Default` maps to `Cursor::Default` |
| `util/cursor.rs:87-89` | `Cursor::Default => null_mut()` |
| `view.rs:382` | `if !cursor.is_null() { addCursorRect:bounds cursor:cursor }` |

A fresh tao window holds `Cursor::Default`, which loads null, and
`reset_cursor_rects` adds nothing for a null cursor. That is the behaviour tao
PR #614 introduced, and it is already in the version we ship.

### The mirroring was structurally inert

tao's content view is not in the window when the app runs. wry builds its own
`WryWebViewParent`, adds the webview to it, then calls
`ns_window.setContentView(parent)`. That evicts the view tao installed. A native
probe replicating both layers in the same order reads back:

```
after tao setContentView: taoView.window=true,  isContentView=true
after wry setContentView: taoView.window=false, taoView.superview=false
```

`Window::set_cursor_icon` stores the icon on that detached view and calls
`invalidateCursorRectsForView:`. AppKit holds no rects for a view outside the
hierarchy. Eight trials set WebKit's I-beam first, then aimed the invalidate at
each candidate in turn:

- the detached tao view, the live content view, the webview, the window itself
- with a stored non-null cursor, and with `disableCursorRects` in force

Every trial read `before=IBEAM after=IBEAM`. So the mirroring neither helped nor
harmed. It cost one IPC call per pointer crossing and moved no glyph.

### The real writer is WebKit, and it declines under four native guards

WebKit sets the cursor itself, and gives up for the whole window whenever any of
four conditions holds
(`Source/WebKit/UIProcess/mac/PageClientImplMac.mm:351`):

| # | Guard | Meaning |
|---|---|---|
| 1 | `!isViewWindowActive()` | the view's window is not the active one |
| 2 | `[NSApp _cursorRectCursor]` | an AppKit cursor rect currently owns the cursor |
| 3 | `!view` or `!window` | the view is not in a window |
| 4 | `windowNumber != windowNumberAtPoint(mouseLocation)` | another window is topmost under the pointer |

Each returns before any `[NSCursor set]`. Each therefore produces exactly the
reported symptom: the plain arrow over everything, text fields included, because
WebKit's I-beam travels the same path as its resize and pointer glyphs.

**None of the four can be reached from CSS or from JavaScript.** That one fact
explains three dead ends at once: the mirroring, `disableCursorRects`, and every
JavaScript workaround the web offers for this symptom. The page is not the layer
that decides.

## Consequences

### What we keep

The revert restores v0.30.1 behaviour exactly, because the thing removed did
nothing. It fixes nothing either, for the same reason. The symptom is not
permanent though: a machine restart cleared it, and the section below records
what that rules out.

### What we give up

The claim that this is fixable from our own code without first proving which
guard fires. It is not.

### The open question, and what the restart ruled out

Which guard fires is still unproven. Two later observations rule out any cause
that belongs to Lucidos alone:

- **A restart cleared it.** The v0.30.4 bundle that showed the arrow now shows
  every cursor correctly, and that bundle never changed. So no code in it is
  the variable, the mirroring it still carries included.
- **Docker Desktop showed the identical symptom over the same period, and the
  same restart cleared it too.** Docker carries none of our code.

So the state was machine-wide, held across app launches and dropped on a boot.
Guards 1 and 4 both read state of that kind. Guard 1 asks which window is
active, guard 4 which window is topmost under the pointer. No application owns
either answer by itself.

This ADR first named two suspects of our own, and both now lose their standing:

- **Guard 2**, through the `NSTitlebarContainerView` resize in
  `traffic_lights.rs` (ADR 0074). It is the only AppKit view we manipulate, and
  a cursor rect is exactly what such a view owns.
- **Guard 4**, through two workspace windows restored to one frame.
  `.window-session.json` restores each window's saved geometry, so two windows
  saved at the same rect land exactly on top of each other. v0.30.4 already
  stops a second window opening for one workspace, and the symptom persisted on
  it. Any remaining guard 4 path would need two different workspaces sharing
  one frame.

Neither mechanism exists in Docker, so neither explains a symptom Docker shared.
Keep them as fallbacks, behind a cheaper question. If it recurs, run three
checks in order, each needing only the packaged app:

1. Does another application show it too? A yes points away from our code.
2. Close one of two overlapping windows, then re-hover a divider (guard 4).
3. Hover a divider without crossing the titlebar band, then again after
   (guard 2).

### The instrument limit, which is why this is not settled

A worktree cannot launch the packaged app, so no probe here can measure the real
window. Two further traps cost this investigation a false negative each, and are
worth knowing before anyone builds the next probe:

- `RunLoop.run(until:)` does not dispatch `NSEvent`s. Only `NSApplication`
  dequeues from the window server, so a probe that merely spins the run loop
  measures a window nothing was delivered to.
- A locked screen puts a shield window over everything. No pointer event exists
  to dispatch, and the hit chain never reaches the probe's own window. Check
  `CGSessionCopyCurrentDictionary()` before trusting a null result.

### Corrections to the record

ADR 0129 flips to Superseded, keeping its text as the record of the wrong turn.
Four of its claims are false, and this ADR corrects each:

- the two-writers premise
- "it covers every element", when `auto` resolves to no rect at all, and `auto`
  is the computed value over most of the app
- the frame-resize risk it named against `disableCursorRects`
- its upstream citations

On the last one: wry #175 was fixed in 2021 by PR #220, and tao #386 is a
Windows frameless-resize issue closed as not planned. Neither was open, and
neither describes this symptom.

## Alternatives considered

**`disableCursorRects` on the window.** ADR 0129 pre-registered this as the
preferred fallback. It is a no-op here. There is no default rect to disable, and
the probe's trials G and H changed no glyph with it in force. Its one named
risk, losing the window frame's edge-resize cursors, is also unfounded: a
controlled sweep of the window's right edge with rects enabled and then disabled
produced byte-identical runs.

**Keep the mirroring in place as harmless.** Rejected. It costs an IPC call per
pointer crossing and an ACL entry, and it keeps a false premise alive in three
file headers. Dead weight that reads as a working mechanism is worse than no
mechanism.

**Chase guard 2 by reverting the traffic-lights surgery.** Rejected as a guess.
That surgery delivers the overlay titlebar the app is built around (ADR 0074).
Removing it to test an unmeasured hypothesis trades a known feature for a maybe.
The two manual checks above cost seconds and come first.

**An upstream tao or wry change.** Premature. Nothing upstream is known to be
broken here, and the two issues ADR 0129 cited as blocking are closed.
