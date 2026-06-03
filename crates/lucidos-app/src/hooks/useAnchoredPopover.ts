import { useEffect, useRef, useState } from 'preact/hooks';
import { clampLeftWithin } from '../utils/dom';

export interface AnchorPosition {
  top: number;
  left: number;
  placement: 'bottom-start' | 'top-start';
}

/** Compute a fixed-positioned popover offset relative to an anchor element.
 *  Defaults to placing the popover *below* the anchor; flips to *above* when
 *  there isn't enough vertical room. Horizontally clamps `left` so the panel
 *  stays inside `container` (or the viewport when no container is given) —
 *  necessary on narrow viewports where an anchor near the right edge would
 *  otherwise push the panel off-screen, and to keep the popover visually
 *  contained within its originating pane. Returns viewport-coordinate offsets
 *  ready for `style.top` / `style.left`. */
export function computeAnchorPosition(
  anchor: HTMLElement,
  panelHeight: number,
  panelWidth: number,
  container?: HTMLElement | null,
): AnchorPosition {
  const rect = anchor.getBoundingClientRect();
  const wantBelow = rect.bottom + panelHeight + 8 <= window.innerHeight;
  const top = wantBelow ? rect.bottom + 4 : rect.top - panelHeight - 4;
  const placement: AnchorPosition['placement'] = wantBelow ? 'bottom-start' : 'top-start';
  const bounds = container?.getBoundingClientRect();
  const left = clampLeftWithin(
    rect.left,
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
      const next = computeAnchorPosition(anchor, panel.offsetHeight, panel.offsetWidth, container);
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
  }, [anchor, panelRef, containerSelector]);
  return pos;
}

/** Build the three handlers (`pointerdown`, `click` capture, `keydown`) that
 *  implement the canonical Lucidos modal dismiss contract:
 *
 *  - `pointerdown` outside the panel+anchor → call `onDismiss`. For
 *    primary-button (left-click / touch / pen) pointerdowns, also arm
 *    "swallow next click" so the paired `click` event the browser is about
 *    to dispatch doesn't fire the underlying element.
 *  - Right-click / middle-click outside still dismisses, but does NOT arm
 *    the suppressor — those buttons dispatch `contextmenu` / `auxclick`,
 *    not `click`, so a stranded flag would swallow a later unrelated
 *    left-click.
 *  - The next `click` (in capture phase) is `stopPropagation`+`preventDefault`d
 *    when the flag is armed. Clicks not preceded by an outside-primary-pointerdown
 *    pass through.
 *  - `Escape` always dismisses.
 *
 *  `onDismiss` may return `false` to declare the call was a no-op (e.g. the
 *  popover is already on its way out via an animation). In that case the
 *  suppressor stays disarmed so the user's tap on a sibling button still
 *  reaches its handler. Returning `void` / `true` keeps the default swallow.
 *
 *  Exported as a pure factory so `.test.ts` can drive the handlers without
 *  jsdom — `useDismissOnOutside` is the hook that wires these to `document`.
 *  See `.claude/rules/frontend.md` § "Modals & popovers: click-outside dismiss". */
export function makeDismissHandlers(
  panelRef: { current: HTMLElement | null },
  anchor: HTMLElement | null,
  onDismiss: () => void | boolean,
): {
  onPointerDown(e: PointerEvent): void;
  onClickCapture(e: MouseEvent): void;
  onKey(e: KeyboardEvent): void;
} {
  let suppressNextClick = false;
  return {
    onPointerDown(e) {
      if (!isOutsidePointerTarget(e.target as Node, panelRef.current, anchor)) return;
      const dismissed = onDismiss();
      if (e.button === 0 && dismissed !== false) suppressNextClick = true;
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
  useEffect(() => {
    if (!isOpen) return;
    const handlers = makeDismissHandlers(panelRef, anchor, () => dismissRef.current());
    document.addEventListener('pointerdown', handlers.onPointerDown, true);
    document.addEventListener('click', handlers.onClickCapture, true);
    document.addEventListener('keydown', handlers.onKey);
    return () => {
      document.removeEventListener('pointerdown', handlers.onPointerDown, true);
      document.removeEventListener('click', handlers.onClickCapture, true);
      document.removeEventListener('keydown', handlers.onKey);
    };
  }, [isOpen, panelRef, anchor]);
}
