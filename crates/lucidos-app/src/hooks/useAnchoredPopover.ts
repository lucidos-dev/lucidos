import { useEffect, useLayoutEffect, useRef, useState } from 'preact/hooks';
import { clampLeftWithin } from '../utils/dom';

export interface AnchorPosition {
  top: number;
  left: number;
  placement: 'bottom-start' | 'top-start';
}

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
 *  `style.left`. */
export function computeAnchorPosition(
  anchor: HTMLElement,
  panelHeight: number,
  panelWidth: number,
  container?: HTMLElement | null,
  align: AnchorAlign = 'start',
): AnchorPosition {
  const rect = anchor.getBoundingClientRect();
  const wantBelow = rect.bottom + panelHeight + 8 <= window.innerHeight;
  const top = wantBelow ? rect.bottom + 4 : rect.top - panelHeight - 4;
  const placement: AnchorPosition['placement'] = wantBelow ? 'bottom-start' : 'top-start';
  const bounds = container?.getBoundingClientRect();
  const desiredLeft = align === 'end' ? rect.right - panelWidth : rect.left;
  const left = clampLeftWithin(
    desiredLeft,
    panelWidth,
    bounds?.left ?? 0,
    bounds?.right ?? window.innerWidth,
  );
  return { top, left, placement };
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
        prev && prev.top === next.top && prev.left === next.left && prev.placement === next.placement
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
    return () => {
      if (rafId !== null) cancelAnimationFrame(rafId);
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
 *  they survive. The first swallowed event (or a `touchcancel`/`pointercancel`,
 *  or a short fuse if the gesture is canceled with no paired event) removes them
 *  all, so a later unrelated tap is never eaten. The `touchend` `preventDefault`
 *  also cancels the synthetic click. Exported for unit tests. */
export function installPairedSwallow(): void {
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
    clearTimeout(fuse);
  }
  function swallow(e: Event) {
    e.stopPropagation();
    e.preventDefault();
    teardown();
  }
  document.addEventListener('touchend', swallow, { capture: true, passive: false });
  document.addEventListener('click', swallow, true);
  document.addEventListener('touchcancel', teardown, { capture: true });
  document.addEventListener('pointercancel', teardown, { capture: true });
  // A canceled gesture may emit no paired event; the fuse keeps a stranded arm
  // from eating a later unrelated tap. Longer than any real tap's down→up.
  fuse = setTimeout(teardown, 1500);
}

/** Build the handlers (`pointerdown`, `touchend` capture, `click` capture,
 *  `keydown`) that implement the canonical Lucidos modal dismiss contract:
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
 *  - `Escape` always dismisses.
 *
 *  `onDismiss` may return `false` to declare the call was a no-op (e.g. the
 *  popover is already on its way out via an animation). In that case the
 *  suppressor stays disarmed (and `onArm` is NOT called) so the user's tap on a
 *  sibling button still reaches its handler. Returning `void` / `true` keeps the
 *  default swallow.
 *
 *  Exported as a pure factory so `.test.ts` can drive the handlers without
 *  jsdom — `useDismissOnOutside` is the hook that wires these to `document`
 *  (and passes `installPairedSwallow` as `onArm`).
 *  See `.claude/rules/frontend.md` § "Modals & popovers: click-outside dismiss". */
export function makeDismissHandlers(
  panelRef: { current: HTMLElement | null },
  anchor: HTMLElement | null,
  onDismiss: () => void | boolean,
  onArm?: () => void,
): {
  onPointerDown(e: PointerEvent): void;
  onTouchEnd(e: TouchEvent): void;
  onClickCapture(e: MouseEvent): void;
  onKey(e: KeyboardEvent): void;
} {
  let suppressNextClick = false;
  return {
    onPointerDown(e) {
      if (!isOutsidePointerTarget(e.target as Node, panelRef.current, anchor)) return;
      const dismissed = onDismiss();
      if (e.button === 0 && dismissed !== false) {
        suppressNextClick = true;
        // Survives the unmount the dismiss is about to trigger — see
        // installPairedSwallow. No-op in unit tests that don't pass onArm.
        onArm?.();
      }
    },
    onTouchEnd(e) {
      // Only the outside-primary-pointerdown path arms the suppressor, so a set
      // flag already means "an outside tap just dismissed". Still re-check the
      // target: never swallow on the anchor (must toggle via its own handler)
      // or inside the panel. preventDefault() here also cancels the synthetic
      // click, so the flag can't strand.
      if (!suppressNextClick) return;
      if (!isOutsidePointerTarget(e.target as Node, panelRef.current, anchor)) return;
      suppressNextClick = false;
      e.stopPropagation();
      e.preventDefault();
    },
    onClickCapture(e) {
      if (suppressNextClick) {
        suppressNextClick = false;
        e.stopPropagation();
        e.preventDefault();
        return;
      }
      // Fallback for `click` events that weren't preceded by an outside
      // pointerdown — e.g. `HTMLElement.click()` (synthetic, common in e2e
      // tests and keyboard-shortcut handlers). The replaced hand-rolled
      // handlers used document click-capture and dismissed + swallowed those
      // too; without this branch the canonical hook silently dropped that
      // contract and any caller relying on synthetic clicks (the thread-filter
      // dropdown e2e tests are the canary) wedged its dismiss flow.
      if (!isOutsidePointerTarget(e.target as Node, panelRef.current, anchor)) return;
      const dismissed = onDismiss();
      if (dismissed !== false) {
        e.stopPropagation();
        e.preventDefault();
      }
    },
    onKey(e) {
      if (e.key === 'Escape') onDismiss();
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
): void {
  // Stash onDismiss in a ref so an inline arrow callback at the call site
  // doesn't churn the effect deps below. Callers should be free to write
  // `() => (open.value = false)` without ceremony; the listeners install
  // once per (isOpen, anchor) transition, not on every render. The ref is
  // updated every render so the latest callback always wins on fire.
  const dismissRef = useRef(onDismiss);
  dismissRef.current = onDismiss;
  // useLayoutEffect (not useEffect) so the document listeners attach
  // synchronously in the same commit that mounts the popover — i.e. BEFORE the
  // browser paints. A plain useEffect attaches a frame later, after paint, which
  // opens a window where the popover is already visible (and dismissible) but no
  // listener is live yet. Any click landing in that gap — a synthetic
  // HTMLElement.click() from a keyboard shortcut or an e2e driver — is missed and
  // the popover wedges open (the thread-filter-dropdown WebKit e2e flake). Pairs
  // with the synthetic-click fallback in makeDismissHandlers: the fallback only
  // helps once the listener exists, so the listener must exist from frame zero.
  useLayoutEffect(() => {
    if (!isOpen) return;
    const handlers = makeDismissHandlers(panelRef, anchor, () => dismissRef.current(), installPairedSwallow);
    document.addEventListener('pointerdown', handlers.onPointerDown, true);
    // Capture phase so this precedes the target button's own bubble-phase
    // `onTouchEnd`; non-passive so `preventDefault()` (which cancels the
    // synthetic click) is honored.
    document.addEventListener('touchend', handlers.onTouchEnd, { capture: true, passive: false });
    document.addEventListener('click', handlers.onClickCapture, true);
    document.addEventListener('keydown', handlers.onKey);
    return () => {
      document.removeEventListener('pointerdown', handlers.onPointerDown, true);
      document.removeEventListener('touchend', handlers.onTouchEnd, true);
      document.removeEventListener('click', handlers.onClickCapture, true);
      document.removeEventListener('keydown', handlers.onKey);
    };
  }, [isOpen, panelRef, anchor]);
}
