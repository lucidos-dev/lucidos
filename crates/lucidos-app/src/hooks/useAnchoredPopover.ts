import { useEffect, useLayoutEffect, useRef, useState } from 'preact/hooks';
import { clampWithin } from '../utils/dom';
import { notePressOutcome } from '../utils/tapGesture';
import { primaryPointerIsDown } from '../utils/pointerPress';

export interface AnchorPosition {
  top: number;
  left: number;
  placement: 'bottom-start' | 'top-start';
  /** Widest the panel may be and still fit inside the box its `left` was
   *  clamped into (the `container`, or the viewport when none was given),
   *  margins already deducted.
   *
   *  Clamping alone only helps a panel that FITS: a panel wider than its
   *  container pins to the container's leading edge and overflows the far one,
   *  which on desktop means a thread-pane popover spilling into the content
   *  pane. A surface that must stay inside its container publishes this as a
   *  CSS var and caps its own width against it.
   *
   *  The first measurement necessarily runs on the UNCAPPED panel, since the
   *  cap is what this value produces. `left` survives that: a too-wide panel
   *  makes `clampWithin` pin it to the container's leading edge, which is
   *  exactly where the panel belongs once it narrows to fill the container.
   *  `top` does NOT, because narrowing reflows the content taller, so the
   *  hook watches the panel's size and re-measures once the cap lands. */
  maxWidth: number;
}

/** Breathing room left at either end of the clamp range. Passed to
 *  `clampWithin` explicitly (rather than leaning on its default) so the margin
 *  the position is clamped by and the margin `maxWidth` deducts cannot drift. */
const CLAMP_MARGIN = 8;

/** Horizontal alignment of the panel relative to its anchor.
 *  - `'start'` (default): the panel's LEFT edge aligns with the anchor's left
 *    edge and the panel grows rightward — the natural fit for a left-positioned
 *    trigger (select-style dropdowns, the message-route button).
 *  - `'end'`: the panel's RIGHT edge aligns with the anchor's right edge and the
 *    panel grows leftward — the conventional fit for an overflow (⋯) trigger
 *    pinned to the far right of a row/header, where `'start'` would push a wide
 *    menu off-screen and the viewport clamp would then strand it near the left
 *    edge, detached from the trigger. */
export type AnchorAlign = 'start' | 'end';

/** Compute a fixed-positioned popover offset relative to an anchor element.
 *  Defaults to placing the popover *below* the anchor; flips to *above* when
 *  there isn't enough vertical room. Horizontally aligns per `align` (see
 *  {@link AnchorAlign}), then clamps `left` so the panel stays inside
 *  `container` (or the viewport when no container is given) — necessary on
 *  narrow viewports where an anchor near the right edge would otherwise push the
 *  panel off-screen, and to keep the popover visually contained within its
 *  originating pane. Returns viewport-coordinate offsets ready for `style.top` /
 *  `style.left`.
 *
 *  `top` is clamped to the viewport for the same reason `left` is, and the case
 *  it covers is the flip: placing a tall panel ABOVE an anchor near the bottom
 *  of a SHORT viewport (a phone in landscape, or one with the virtual keyboard
 *  open shrinking the visual viewport) produces a negative `top`, which pushes
 *  the panel's head off the screen where its content cannot be scrolled back
 *  into view. The clamp never re-flips: a panel too tall for the space pins to
 *  the top margin and overlaps its anchor, which is strictly better than being
 *  unreachable. Surfaces should still cap their own `max-height` against the
 *  viewport so that overlap stays rare (see `.prompt-bar-popover`). */
export function computeAnchorPosition(
  anchor: HTMLElement,
  panelHeight: number,
  panelWidth: number,
  container?: HTMLElement | null,
  align: AnchorAlign = 'start',
): AnchorPosition {
  const rect = anchor.getBoundingClientRect();
  const wantBelow = rect.bottom + panelHeight + CLAMP_MARGIN <= window.innerHeight;
  const desiredTop = wantBelow ? rect.bottom + 4 : rect.top - panelHeight - 4;
  const top = clampWithin(desiredTop, panelHeight, 0, window.innerHeight, CLAMP_MARGIN);
  const placement: AnchorPosition['placement'] = wantBelow ? 'bottom-start' : 'top-start';
  const bounds = container?.getBoundingClientRect();
  const boundsLeft = bounds?.left ?? 0;
  const boundsRight = bounds?.right ?? window.innerWidth;
  const desiredLeft = align === 'end' ? rect.right - panelWidth : rect.left;
  const left = clampWithin(desiredLeft, panelWidth, boundsLeft, boundsRight, CLAMP_MARGIN);
  // A container with no room left to give is not a cap, it is a disappearing
  // act: capping to 0 renders a zero-width panel that is invisible while the
  // overlay is still open and holding the UI behind it inert. The thread pane
  // reaches exactly that when a keyboard shortcut collapses it with a popover
  // already open, so a degenerate container falls back to the viewport, the
  // same box used when no container was given at all.
  const containerFit = boundsRight - boundsLeft - 2 * CLAMP_MARGIN;
  const maxWidth = containerFit > 0 ? containerFit : Math.max(0, window.innerWidth - 2 * CLAMP_MARGIN);
  return { top, left, placement, maxWidth };
}

/** Decide whether a pointerdown should dismiss the popover. Clicks on the panel
 *  itself or on the anchor element are kept inside — the anchor is excluded so
 *  re-clicking it can toggle the popover via the caller's click handler instead
 *  of being eaten by this dismiss handler firing first. */
export function isOutsidePointerTarget(
  target: Node,
  panel: HTMLElement | null,
  anchor: HTMLElement | null,
): boolean {
  if (panel?.contains(target)) return false;
  if (anchor?.contains(target)) return false;
  return true;
}

/** Track an anchored popover's position and keep it pinned to the anchor as the
 *  page scrolls or resizes. Returns the current viewport offsets, or `null`
 *  when the popover is closed (`anchor === null`).
 *
 *  Recomputes on scroll, on resize (window and visual viewport) and when the
 *  PANEL itself changes size, since its height is an input to the position.
 *  rAF-coalesced + equality-guarded so a fast scroll burst produces at most one
 *  recompute per frame and no re-render when the anchor's screen position
 *  hasn't actually changed (common during inertia scroll where anchor and
 *  scroll container move together). Passive scroll listener so we don't block
 *  the chat's auto-scroll. */
export function useAnchoredPosition(
  anchor: HTMLElement | null,
  panelRef: { current: HTMLElement | null },
  containerSelector?: string,
  align: AnchorAlign = 'start',
): AnchorPosition | null {
  const [pos, setPos] = useState<AnchorPosition | null>(null);
  useEffect(() => {
    if (!anchor) {
      setPos(null);
      return;
    }
    const container = containerSelector ? anchor.closest<HTMLElement>(containerSelector) : null;
    let rafId: number | null = null;
    const recompute = () => {
      rafId = null;
      const panel = panelRef.current;
      if (!panel) return;
      const next = computeAnchorPosition(anchor, panel.offsetHeight, panel.offsetWidth, container, align);
      setPos(prev =>
        prev &&
        prev.top === next.top &&
        prev.left === next.left &&
        prev.placement === next.placement &&
        prev.maxWidth === next.maxWidth
          ? prev
          : next,
      );
    };
    const schedule = () => {
      if (rafId !== null) return;
      rafId = requestAnimationFrame(recompute);
    };
    recompute();
    // Capture-phase: the chat pane is its own scroll container, not window — bubbling
    // scrolls would never reach a non-capture window listener.
    window.addEventListener('scroll', schedule, { capture: true, passive: true });
    window.addEventListener('resize', schedule);
    // visualViewport tracks the mobile virtual keyboard. On iOS Safari with
    // `interactive-widget=resizes-visual` (Lucidos's viewport meta), neither
    // `window.resize` nor a `window.scroll` fires when the keyboard appears
    // or dismisses — only `visualViewport.resize` does. MobileSwipeContainer
    // reads `visualViewport.height` into the `--app-height` CSS var, which
    // reflows the entire `.app-shell`; without these listeners the anchor's
    // `getBoundingClientRect` changes on keyboard close and the popover stays
    // pinned to its keyboard-open coordinates (the "left lying after keyboard
    // close" case). `visualViewport.scroll` covers iOS visual-viewport panning
    // (pinch-zoom drag) while the popover is open.
    const vv = window.visualViewport;
    if (vv) {
      vv.addEventListener('resize', schedule);
      vv.addEventListener('scroll', schedule);
    }
    // The panel's own size is an INPUT to the position (`top` is derived from
    // its height whenever it opens upward off the bottom-docked prompt bar), so
    // it has to be watched like the viewport is. Two ways it moves under us:
    // the caller applies `maxWidth` as a cap, which narrows the panel and
    // reflows its text onto more lines, and the content itself changes while
    // open (a wait resolves, a todo row lands). Without this the first
    // measurement is the only one until an unrelated scroll or resize, and an
    // upward-opening panel that grew after being measured hangs down over the
    // anchor that opened it. The observer's initial callback is free: an
    // unchanged position hits the equality guard above and re-renders nothing,
    // and a position change cannot itself change the panel's size, so this
    // cannot cycle.
    const ro = typeof ResizeObserver === 'undefined' ? null : new ResizeObserver(schedule);
    if (ro && panelRef.current) ro.observe(panelRef.current);
    return () => {
      if (rafId !== null) cancelAnimationFrame(rafId);
      ro?.disconnect();
      window.removeEventListener('scroll', schedule, true);
      window.removeEventListener('resize', schedule);
      if (vv) {
        vv.removeEventListener('resize', schedule);
        vv.removeEventListener('scroll', schedule);
      }
    };
  }, [anchor, panelRef, containerSelector, align]);
  return pos;
}

/** Install one-shot, capture-phase `touchend` + `click` swallowers on `document`
 *  that **outlive the dismissing overlay**. An outside-primary-pointerdown
 *  dismiss closes the overlay, which re-renders and tears down the overlay's own
 *  (open-gated) listeners — and that teardown runs in the microtask checkpoint
 *  BETWEEN the pointerdown task and the next event task, i.e. BEFORE the browser
 *  dispatches the paired `touchend` (touch) / `click` (mouse) of the same
 *  gesture. So the open-gated `onTouchEnd`/`onClickCapture` below are already
 *  gone by the time the paired event fires; the swallow has to live somewhere
 *  the unmount can't reach. These listeners aren't tied to any component, so
 *  they survive. The `touchend` `preventDefault` also cancels the synthetic
 *  click. Exported for unit tests.
 *
 *  The arm belongs to ONE gesture, `arming`, and `onNewGesture` below is what
 *  holds it to that. */
export function installPairedSwallow(arming?: Event): void {
  if (typeof document === 'undefined') return;
  let done = false;
  let fuse: ReturnType<typeof setTimeout>;
  function teardown() {
    if (done) return;
    done = true;
    document.removeEventListener('touchend', swallow, true);
    document.removeEventListener('click', swallow, true);
    document.removeEventListener('touchcancel', teardown, true);
    document.removeEventListener('pointercancel', teardown, true);
    document.removeEventListener('pointerdown', onNewGesture, true);
    clearTimeout(fuse);
  }
  // `notePressOutcome` is what stops this reading as a DEAD press. The
  // `stopPropagation` skips every bubble-phase observer on `document`, the
  // dead-press probe included, so the swallow has to name itself.
  function swallow(e: Event) {
    notePressOutcome('swallowed');
    e.stopPropagation();
    e.preventDefault();
    teardown();
  }
  /** The bound on the arm, and the reason it is a gesture rather than a target
   *  or a point. The paired event of a dismissing tap can land anywhere: the
   *  node under the finger is often the one the dismiss just re-rendered. A new
   *  `pointerdown` proves the arming gesture is over. The arm dies there, in
   *  the capture phase, ahead of the new gesture's own `touchend`.
   *
   *  An arm that outlives its gesture eats an unrelated tap, which is a dead
   *  button. The way it strands is ordinary: a `touchend` dispatched to a node
   *  the dismiss REMOVED never reaches `document`, and no cancel fires either.
   *  See `docs/plans/2026-08-28-a-swallowed-tap-says-so.md`.
   *
   *  The arming pointerdown is still being dispatched while this listener is
   *  added. A DOM dispatch iterates a COPY of the listener list, so no browser
   *  delivers it here. The test document stub iterates the live one, and
   *  comparing the event object holds in both.
   *
   *  A SECOND finger is not a new gesture. Its pointerdown carries
   *  `isPrimary: false`, and tearing down on it would let the first finger's
   *  paired event through, which is the contract this whole one-shot upholds. */
  function onNewGesture(e: Event) {
    if (e === arming) return;
    if ((e as PointerEvent).isPrimary === false) return;
    teardown();
  }
  document.addEventListener('touchend', swallow, { capture: true, passive: false });
  document.addEventListener('click', swallow, true);
  document.addEventListener('touchcancel', teardown, { capture: true });
  document.addEventListener('pointercancel', teardown, { capture: true });
  document.addEventListener('pointerdown', onNewGesture, { capture: true });
  // Backstop for a page nobody touches again, not the bound. A reflexive
  // second tap arrives long inside it, and `onNewGesture` catches that one.
  fuse = setTimeout(teardown, 1500);
}

/** Build the handlers (`pointerdown`, `touchend` capture, `click` capture,
 *  `click` bubble, `keydown`) that implement the canonical Lucidos modal
 *  dismiss contract:
 *
 *  - `pointerdown` outside the panel+anchor → call `onDismiss`. For
 *    primary-button (left-click / touch / pen) pointerdowns, also arm
 *    "swallow next paired event" so the `touchend`/`click` the browser is
 *    about to dispatch doesn't fire the underlying element. Arming has two
 *    halves: a local `suppressNextClick` flag (consumed by `onTouchEnd` /
 *    `onClickCapture` below when those listeners are still alive — the
 *    same-task / synthetic-driver case) AND `onArm()`, which installs the
 *    overlay-outliving one-shot (`installPairedSwallow`) for the common case
 *    where the dismiss's re-render has already torn these handlers down before
 *    the paired event fires. The two are complementary, not redundant: the
 *    local flag covers same-task swallows, `onArm` covers cross-task ones.
 *  - Right-click / middle-click outside still dismisses, but does NOT arm
 *    the suppressor — those buttons dispatch `contextmenu` / `auxclick`,
 *    not `click`, so a stranded flag would swallow a later unrelated
 *    left-click.
 *  - `touchend` (in capture phase) outside the panel+anchor, when the flag is
 *    armed, is `stopPropagation`+`preventDefault`d and disarms. This covers
 *    touch buttons that run their action on `onTouchEnd` and `preventDefault()`
 *    the synthetic click (the iOS keyboard-nudge pattern in `composeHandlers`):
 *    the outside pointerdown dismisses the overlay but the button's own
 *    `touchend` would otherwise fire the action on the same tap, and — because
 *    the button cancels the synthetic click — no `click` ever arrives. The
 *    capture phase precedes the target button's bubble-phase `onTouchEnd`, so
 *    the swallow wins. (When the re-render has removed this handler first, the
 *    `onArm` one-shot does the same job.)
 *  - The next `click` (in capture phase) is `stopPropagation`+`preventDefault`d
 *    when the flag is armed. Clicks not preceded by an outside-primary-pointerdown
 *    pass through. On touch the `touchend` already consumed the flag, so the
 *    click path is the mouse case.
 *  - A click dispatched *within* an inside click's dispatch (a menu item whose
 *    handler calls `someInput.click()`) is treated as inside too, so the
 *    synthetic-click fallback never swallows a popover's own action. The window
 *    is that one dispatch: `onClickCapture` opens it on the inside click and
 *    `onClickBubble` closes it when that same event reaches document on the way
 *    back up (with a task-scoped backstop for a handler that stops
 *    propagation). See the comments at those branches for the file-picker case
 *    it exists for.
 *  - `Escape` dismisses when this overlay is the top panel. It is the fallback
 *    path: the central capture-phase dispatcher normally gets there first.
 *
 *  `onDismiss` may return `false` to declare the call was a no-op (e.g. the
 *  popover is already on its way out via an animation). In that case the
 *  suppressor stays disarmed (and `onArm` is NOT called) so the user's tap on a
 *  sibling button still reaches its handler. Returning `void` / `true` keeps the
 *  default swallow.
 *
 *  **`isTop` is what makes STACKED overlays behave.** Each open overlay installs
 *  its own document listener, and each asks only whether the target is outside
 *  ITS panel. So without this, a click on the upper overlay reads as an outside
 *  click to the lower one, which then closes invisibly behind it. Nesting hid
 *  that for years: a dropdown inside a modal sits physically inside the modal's
 *  panel, so the modal never saw it as outside. Two SIBLING overlays are the
 *  case that exposes it, and a condition modal opened from the waiting panel is
 *  exactly that.
 *
 *  The caller answers it with the top PANEL rather than the raw `overlayStack`
 *  top, and `topPanelOverlay` says why. The CENTRAL Escape dispatcher keeps the
 *  raw top, which is what lets a step inside a panel answer the key first. The
 *  `onKey` below is only its fallback, so it follows `isTop` like the rest.
 *
 *  **`openedUnderPress` is the gesture-opened case, and it is not the fallback
 *  above.** An overlay a long press opens has no anchor to exempt, and its
 *  opening `pointerdown` fired before these listeners existed. That same
 *  gesture's trailing click therefore arrives unpaired and reads as synthetic,
 *  so the lift dismissed the menu the hold had just opened. Told that a press
 *  was already down at open time, the handlers spend that one click and
 *  dismiss nothing. `primaryPointerIsDown` answers it, and counts only TRUSTED
 *  presses, since only those get a click from the browser.
 *
 *  Exported as a pure factory so `.test.ts` can drive the handlers without
 *  jsdom — `useDismissOnOutside` is the hook that wires these to `document`
 *  (and passes `installPairedSwallow` as `onArm`).
 *  See `.claude/rules/frontend.md` § "Modals & popovers: click-outside dismiss". */
export function makeDismissHandlers(
  panelRef: { current: HTMLElement | null },
  anchor: HTMLElement | null,
  onDismiss: () => void | boolean,
  onArm?: (arming: Event) => void,
  isTop: () => boolean = () => true,
  openedUnderPress = false,
): {
  onPointerDown(e: PointerEvent): void;
  onTouchEnd(e: TouchEvent): void;
  onCancel(): void;
  onClickCapture(e: MouseEvent): void;
  onClickBubble(e: MouseEvent): void;
  onKey(e: KeyboardEvent): void;
} {
  let suppressNextClick = false;
  // The INSIDE click currently being dispatched, or null. Held as the event
  // OBJECT rather than a boolean so the window closes on exactly that
  // dispatch: `onClickBubble` clears it when the same event reaches document
  // on the way back up. See `onClickCapture` for what it protects.
  let insideClick: MouseEvent | null = null;
  // An outside PRIMARY pointerdown reached this handler and has not been paired
  // with its click yet, INCLUDING one declined for not being the top panel. The
  // fallback below exists for a click with no pointerdown behind it, and a
  // declined pointerdown is still a pointerdown.
  //
  // Without it a stacked pair leaked: the upper overlay consumes the gesture
  // and unmounts on the microtask, so by the time the paired click lands, this
  // one IS top. It read the click as unpaired and closed too, which is the
  // whole thing `isTop` was added to prevent, one task later.
  //
  // Primary-only for the same reason `suppressNextClick` is. A secondary button
  // dispatches `contextmenu` or `auxclick` and never a `click`, so the pairing
  // would never arrive. The flag would strand set and eat the next synthetic
  // click's dismiss. `onCancel` covers the other way a gesture ends clickless.
  let awaitingPairedClick = false;
  // The click that pairs with the press that OPENED this overlay has not
  // arrived yet. A GESTURE-opened overlay has no anchor to exempt (see
  // `OverflowMenu`'s `openRef`), and its opening `pointerdown` fired before
  // these listeners existed. So its trailing click reaches the fallback below
  // looking exactly like a synthetic one, and the lift that opened the menu
  // dismissed it again.
  //
  // Held apart from `awaitingPairedClick`, which `onTouchEnd` clears. That
  // clear is right for a DISMISSING tap, whose paired click the swallow
  // cancels. It is wrong here: this gesture's click is not cancelled and is
  // still coming.
  let owedToOpeningPress = openedUnderPress;
  return {
    onPointerDown(e) {
      // A new press ends the opening gesture, whichever side of the panel it
      // lands on. Ahead of the inside/outside test for that reason.
      if (e.isPrimary !== false) owedToOpeningPress = false;
      if (!isOutsidePointerTarget(e.target as Node, panelRef.current, anchor)) return;
      if (e.button === 0) awaitingPairedClick = true;
      if (!isTop()) return;
      const dismissed = onDismiss();
      if (e.button === 0 && dismissed !== false) {
        suppressNextClick = true;
        // Survives the unmount the dismiss is about to trigger — see
        // installPairedSwallow. No-op in unit tests that don't pass onArm.
        // The event goes with it: the arm belongs to THIS gesture and nothing
        // later can identify it otherwise.
        onArm?.(e);
      }
    },
    onTouchEnd(e) {
      // Only the outside-primary-pointerdown path arms the suppressor, so a set
      // flag already means "an outside tap just dismissed". Still re-check the
      // target: never swallow on the anchor (must toggle via its own handler)
      // or inside the panel. preventDefault() here also cancels the synthetic
      // click, so the flag can't strand.
      if (!isOutsidePointerTarget(e.target as Node, panelRef.current, anchor)) return;
      // The touch gesture ends here, and the click it would pair with is
      // cancelled: by the `preventDefault()` below when this handler armed, and
      // by the TOP overlay's paired swallow when it declined. Either way
      // nothing later clears the pairing, so it is cleared now.
      awaitingPairedClick = false;
      if (!suppressNextClick) return;
      suppressNextClick = false;
      // Same reason as `installPairedSwallow`'s copy: this stops propagation at
      // `document` in the capture phase, so the press has to name itself or it
      // reads as dead to every observer downstream.
      notePressOutcome('swallowed');
      e.stopPropagation();
      e.preventDefault();
    },
    // A gesture the browser took away (a scroll, a drag, a lost pointer) ends
    // with no click at all. BOTH flags waiting for one are then stale.
    //
    // The pairing would strand on an overlay that declined the pointerdown and
    // stayed open. The swallow would strand on one that dismissed and stayed
    // MOUNTED, which the drawer does by design during its slide-out: it then
    // ate a neighbour's tap, the very thing its `false` return exists to stop.
    onCancel() {
      awaitingPairedClick = false;
      suppressNextClick = false;
      // A cancelled gesture dispatches no click at all, the opening one
      // included. Left set, the expectation would eat the next click's
      // dismiss.
      owedToOpeningPress = false;
    },
    onClickCapture(e) {
      const paired = awaitingPairedClick;
      awaitingPairedClick = false;
      // Consumed here whatever the branch below does: the opening gesture has
      // exactly one click, and this is it.
      const opening = owedToOpeningPress;
      owedToOpeningPress = false;
      if (suppressNextClick) {
        suppressNextClick = false;
        e.stopPropagation();
        e.preventDefault();
        return;
      }
      if (!isOutsidePointerTarget(e.target as Node, panelRef.current, anchor)) {
        // An INSIDE click. Anything it dispatches while it is still unwinding
        // is a consequence of it, not a new outside click, so hold it open and
        // let the nested event through below.
        //
        // What that protects: a menu item whose action is `someInput.click()`
        // on an element outside the panel. The composer's attach menu is the
        // one in the tree: its File item clicks the persistent hidden
        // `<input type="file">`, which lives in `.prompt-box` rather than in
        // the panel precisely so the menu's re-render can't unmount it
        // mid-tap. Without this, the fallback below read that nested click as
        // an outside one and `preventDefault()`d it, and showing the file
        // chooser is the CANCELABLE DEFAULT ACTION of a click on a file input,
        // so the item did nothing at all, with nothing logged or shown. The
        // click has to stay synchronous inside the user gesture (deferring it
        // drops transient activation, which is what makes a file picker
        // unreliable on iOS), so the contract is what gives, not the caller.
        //
        // The window is exactly this dispatch: `onClickBubble` closes it when
        // this same event reaches document on the way back up. The timer is
        // only a BACKSTOP for a target handler that `stopPropagation()`s, so
        // the bubble never arrives, and it is a task rather than a microtask
        // because a microtask checkpoint runs every time the JS stack empties
        // (between two listeners of one dispatch included), which is early
        // enough to have shipped the bug this fixes: cleared on a microtask,
        // the mark was gone before the item's own handler ran, the picker
        // stayed shut, and a unit test calling the two handlers back-to-back
        // with no checkpoint between them still passed.
        insideClick = e;
        setTimeout(() => { if (insideClick === e) insideClick = null; }, 0);
        return;
      }
      if (insideClick) return;
      // This click HAS a pointerdown behind it, and the branch below is for the
      // ones that do not. Reaching here means the overlay above consumed that
      // pointerdown. So the gesture was never meant for this overlay, however
      // the stack looks now that the upper one has gone.
      if (paired) return;
      // The tail of the gesture that opened this overlay. It has a pointerdown
      // behind it too; that one simply predates these listeners.
      if (opening) return;
      if (!isTop()) return;
      // Fallback for `click` events that weren't preceded by an outside
      // pointerdown — e.g. `HTMLElement.click()` (synthetic, common in e2e
      // tests and keyboard-shortcut handlers). The replaced hand-rolled
      // handlers used document click-capture and dismissed + swallowed those
      // too; without this branch the canonical hook silently dropped that
      // contract and any caller relying on synthetic clicks (the thread-filter
      // dropdown e2e tests are the canary) wedged its dismiss flow.
      const dismissed = onDismiss();
      if (dismissed !== false) {
        e.stopPropagation();
        e.preventDefault();
      }
    },
    onClickBubble(e) {
      // Bubble phase at document, so this is the last thing that runs for an
      // inside click: the dispatch is over and anything it was going to
      // dispatch has been dispatched. Matching on the event OBJECT is what
      // keeps the window tight rather than merely short. A boolean cleared on
      // a timer would exempt every click in the rest of the task, so a
      // handler doing `insidebutton.click(); outsideButton.click()` would get
      // its second click exempted too; and a boolean cleared on the FIRST
      // bubble would close the window on the nested click's own trip up here,
      // leaving a second nested click swallowed.
      if (insideClick === e) insideClick = null;
    },
    onKey(e) {
      if (e.key === 'Escape' && isTop()) onDismiss();
    },
  };
}

/** Wires "dismiss on click-outside / Escape" for an anchored popover. Scroll and
 *  resize do NOT dismiss — pair with `useAnchoredPosition` if the popover should
 *  follow its anchor through page scrolling. Caller owns open/close state.
 *
 *  The dismissing click is **swallowed** — see `makeDismissHandlers` for the
 *  full contract. The anchor is exempted (re-clicking it must toggle the
 *  popover via the caller's onClick), so the toggle path continues to fire as
 *  a normal click. */
export function useDismissOnOutside(
  isOpen: boolean,
  panelRef: { current: HTMLElement | null },
  anchor: HTMLElement | null,
  onDismiss: () => void | boolean,
  isTop?: () => boolean,
): void {
  // Stash onDismiss in a ref so an inline arrow callback at the call site
  // doesn't churn the effect deps below: the listeners install once per
  // (isOpen, anchor) transition, not on every render. The ref is updated every
  // render so the latest callback always wins on fire.
  //
  // Give that callback a BRACED body, `() => { open.value = false; }`. This
  // comment used to recommend the expression form, and that was a bug it
  // spread to four live overlays. An arrow with an expression body returns the
  // assignment's value, so `() => (open.value = false)` returns `false`, which
  // is the no-op signal below: the overlay closed and the paired click was
  // never swallowed, so the control underneath it fired on the dismissing tap.
  const dismissRef = useRef(onDismiss);
  dismissRef.current = onDismiss;
  // Same ref treatment, and for a second reason: the predicate reads the
  // overlay stack, so it must be evaluated when the event fires rather than
  // captured when the listeners installed.
  const isTopRef = useRef(isTop);
  isTopRef.current = isTop;
  // useLayoutEffect (not useEffect) so the document listeners attach
  // synchronously in the same commit that mounts the popover — i.e. BEFORE the
  // browser paints. A plain useEffect attaches a frame later, after paint, which
  // opens a window where the popover is already visible (and dismissible) but no
  // listener is live yet. Any click landing in that gap — a synthetic
  // HTMLElement.click() from a keyboard shortcut or an e2e driver — is missed and
  // the popover wedges open (the WebKit e2e flake the thread filter hit back
  // when it was still an anchored dropdown; it is a pane panel now). Pairs
  // with the synthetic-click fallback in makeDismissHandlers: the fallback only
  // helps once the listener exists, so the listener must exist from frame zero.
  useLayoutEffect(() => {
    if (!isOpen) return;
    const handlers = makeDismissHandlers(
      panelRef,
      anchor,
      () => dismissRef.current(),
      installPairedSwallow,
      () => isTopRef.current?.() ?? true,
      // Asked HERE, as the overlay opens, because that is the only moment the
      // answer means anything: a press still down now is the one that opened
      // this overlay. See `makeDismissHandlers` on `openedUnderPress`.
      primaryPointerIsDown(),
    );
    document.addEventListener('pointerdown', handlers.onPointerDown, true);
    // Capture phase so this precedes the target button's own bubble-phase
    // `onTouchEnd`; non-passive so `preventDefault()` (which cancels the
    // synthetic click) is honored.
    document.addEventListener('touchend', handlers.onTouchEnd, { capture: true, passive: false });
    document.addEventListener('click', handlers.onClickCapture, true);
    // Bubble phase, deliberately: it is the far end of the same dispatch the
    // capture listener above opened, and closing the nested-click window there
    // is what keeps the window one dispatch wide instead of one task wide.
    document.addEventListener('click', handlers.onClickBubble);
    document.addEventListener('keydown', handlers.onKey);
    // Pairs with `installPairedSwallow`'s own cancel teardown. That one drops
    // the surviving one-shot; this one drops the two local flags waiting for
    // the same click. A gesture can end without the click it announced.
    document.addEventListener('pointercancel', handlers.onCancel, true);
    document.addEventListener('touchcancel', handlers.onCancel, true);
    return () => {
      document.removeEventListener('pointerdown', handlers.onPointerDown, true);
      document.removeEventListener('touchend', handlers.onTouchEnd, true);
      document.removeEventListener('click', handlers.onClickCapture, true);
      document.removeEventListener('click', handlers.onClickBubble);
      document.removeEventListener('keydown', handlers.onKey);
      document.removeEventListener('pointercancel', handlers.onCancel, true);
      document.removeEventListener('touchcancel', handlers.onCancel, true);
    };
  }, [isOpen, panelRef, anchor]);
}
