import { useRef, useLayoutEffect } from 'preact/hooks';
import { createPortal } from 'preact/compat';
import type { ComponentChildren, RefObject, JSX } from 'preact';
import { useDismissOnOutside } from '../../hooks/useAnchoredPopover';
import { pushOverlay, removeOverlay, topPanelOverlay } from '../../store/overlayStack';

let overlayIdCounter = 0;

// While any <Overlay> is open, the children of the main UI (`.app-shell > *`)
// are made inert via `data-overlay-open` on <html> + CSS — so elements BEHIND
// the overlay don't hover-highlight or activate; a click just lands on
// `.app-shell` and dismisses the overlay. This is the hover analog of the
// outside-click swallow: the overlay is the focused thing, so the stuff behind
// it shouldn't react. The overlay panels stay interactive (re-enabled in CSS via
// `[data-overlay-panel]`), and the toggle that opened the overlay stays
// clickable via `[data-overlay-anchor]` (see `markAnchorInteractive` below) so
// re-activating it closes via its OWN handler. `Toast` / the modal scrims render
// OUTSIDE `.app-shell` so they're unaffected. Ref-counted so stacked overlays
// keep it set until the last closes.
let openOverlayCount = 0;
function acquireOverlayInert(): void {
  if (typeof document === 'undefined') return;
  openOverlayCount += 1;
  if (openOverlayCount === 1) document.documentElement.setAttribute('data-overlay-open', '');
}
function releaseOverlayInert(): void {
  if (typeof document === 'undefined') return;
  openOverlayCount = Math.max(0, openOverlayCount - 1);
  if (openOverlayCount === 0) document.documentElement.removeAttribute('data-overlay-open');
}

// The anchor toggle lives inside `.app-shell`, whose children go inert while the
// overlay is open. Mark it so CSS keeps it `pointer-events: auto` — otherwise the
// anchor inherits the inert and re-activating it can't fire its own handler (the
// click would route through the outside-dismiss path, and Playwright/a careful
// user can't land on it at all). Returns the cleanup that unmarks the same node.
function markAnchorInteractive(anchor: HTMLElement | null): () => void {
  if (!anchor) return () => {};
  anchor.setAttribute('data-overlay-anchor', '');
  return () => anchor.removeAttribute('data-overlay-anchor');
}

export interface OverlayProps {
  /** Whether the overlay is open. The dismiss/Escape listeners install only
   *  while true; when false the overlay unmounts (or renders hidden, see
   *  `keepMounted`). */
  open: boolean;
  /** Close the overlay — flip the signal that renders it. Called on
   *  outside-click, on Escape (via the central overlay stack), or by the
   *  caller's own toggle handler. */
  onClose: () => void;
  /** The toggle element that opened this overlay. Exempt from the
   *  outside-pointerdown dismiss so re-activating it closes via its OWN handler
   *  rather than being raced by the dismiss (on touch the dismiss-then-reopen
   *  race is the SearchEverywhere `anchor=null` bug). Pass the toggle for any
   *  overlay opened from a button; omit / `null` for a backdrop-only modal that
   *  has no toggle. */
  anchor?: HTMLElement | null;
  /** `false` → render the panel on its own with no full-screen `.modal-overlay`
   *  container — for anchored popovers/dropdowns that position the panel
   *  themselves (pass the computed offsets via `panelStyle`). Default `true`
   *  (centered/sheet modal with a backdrop container). */
  backdrop?: boolean;
  /** Extra class on the full-screen `.modal-overlay` container (backdrop mode) —
   *  e.g. top-alignment or click-through (`pointer-events: none`). */
  overlayClass?: string;
  /** Class on the panel box — the element that owns `pointer-events` and holds
   *  the content. The dismiss hook treats clicks inside it as "inside". */
  panelClass?: string;
  /** Inline style on the panel — anchored placement passes the computed
   *  `top`/`left` here. */
  panelStyle?: string | JSX.CSSProperties;
  /** ARIA role on the panel. Omitted by default — pass `'dialog'` for
   *  centered/sheet modals; anchored popovers/dropdowns leave it unset so the
   *  panel keeps its plain-`div` semantics (a dropdown menu is not a dialog). */
  panelRole?: JSX.HTMLAttributes<HTMLDivElement>['role'];
  /** `aria-modal` on the panel (backdrop modals that trap focus). */
  ariaModal?: boolean;
  /** `data-role` on the panel — dual-layout-safe querying (never use `id`). */
  dataRole?: string;
  /** Extra attributes/handlers spread onto the panel element — `aria-label`,
   *  `onScroll` (infinite-scroll dropdowns), `onKeyDown` (keyboard-nav menus),
   *  `onAnimationEnd` (slide-out drawers). The curated props above
   *  (`class`/`style`/`role`/`aria-modal`/`data-role`/`ref`) win over anything
   *  passed here. Use for panel concerns that aren't the dismiss contract. */
  panelProps?: JSX.HTMLAttributes<HTMLDivElement>;
  /** Keep the overlay element mounted (hidden) when closed instead of
   *  unmounting. iOS PWA: avoids ghost pixels from a `will-change` compositing
   *  layer churning on unmount. Default `false`. */
  keepMounted?: boolean;
  /** Class on the closed-but-mounted container (`keepMounted` mode). */
  hiddenClass?: string;
  /** External ref to the panel — anchored callers pass this to
   *  `useAnchoredPosition` so it can measure the panel. Omit to use an internal
   *  ref (the dismiss hook still gets it either way). */
  panelRef?: RefObject<HTMLDivElement>;
  /** Render the panel into `document.body` instead of inline. REQUIRED for an
   *  anchored (`backdrop:false`) popover whose host lives under a `transform`ed
   *  / `filter`ed ancestor (e.g. the floating `.app-header` regions, which use
   *  `transform: translateY(-50%)`): a transform makes that ancestor the
   *  containing block for `position: fixed` descendants, so the panel's
   *  viewport coordinates would resolve against the ancestor and render
   *  off-screen. Portaling to `<body>` restores the viewport as the containing
   *  block. The dismiss/anchor/Escape/inert contracts are unaffected —
   *  `panel.contains()` works across a portal, and the portaled node sits
   *  outside `.app-shell` so it's interactive without the inert exemption.
   *  Only honored for `backdrop:false`. Default `false`. */
  portal?: boolean;
  children: ComponentChildren;
}

/** The one component every overlay renders through — modal, popover, dropdown,
 *  command palette, anchored panel, bottom sheet. It owns the full
 *  click-outside dismiss contract so individual overlays can't mis-wire it:
 *
 *  1. **Dismiss + swallow** — an outside `pointerdown` closes the overlay AND
 *     swallows the paired `click`, so the element under the cursor never also
 *     fires (`useDismissOnOutside` / `makeDismissHandlers`).
 *  2. **Anchor exemption** — re-activating the `anchor` toggle closes via the
 *     toggle's own handler (it isn't treated as "outside"), never reopening.
 *  3. **Escape** — registered into the central LIFO `overlayStack`; the single
 *     capture-phase dispatcher in `useKeyboardShortcuts` pops the top entry and
 *     `stopPropagation`s, which also shadows the hook's own (bubble-phase)
 *     Escape so `onClose` fires exactly once.
 *
 *  Positioning stays with the caller (CSS via `overlayClass`/`panelClass` for
 *  modals, `panelStyle` for anchored popovers) — only the contract is
 *  centralized. See `.claude/rules/frontend.md` § "Modals & Popovers". */
export function Overlay({
  open,
  onClose,
  anchor = null,
  backdrop = true,
  overlayClass,
  panelClass,
  panelStyle,
  panelRole,
  ariaModal,
  dataRole,
  panelProps,
  keepMounted = false,
  hiddenClass,
  panelRef: externalPanelRef,
  portal = false,
  children,
}: OverlayProps) {
  // Portal only makes sense for a backdrop-less anchored panel (a backdrop
  // modal owns the full-screen `.modal-overlay` container instead).
  const usePortal = portal && !backdrop && typeof document !== 'undefined';
  const internalRef = useRef<HTMLDivElement>(null);
  const panelRef = externalPanelRef ?? internalRef;

  // This overlay's `overlayStack` identity. Declared before the dismiss hook
  // below, which reads it to ask whether this overlay is the top one.
  const idRef = useRef<string>();
  if (idRef.current === undefined) idRef.current = `overlay-${++overlayIdCounter}`;

  // Click-outside dismiss + swallow; the anchor (if any) is exempt, and only
  // the topmost PANEL answers. Two SIBLING overlays each read the other's panel
  // as outside their own. So without the check, a click on the upper one closes
  // the lower one behind it.
  //
  // `topPanelOverlay`, never the raw stack top: an Escape-only registrant draws
  // nothing, so a pointer cannot be meant for it, and one sits ABOVE its own
  // host panel by design (`ModelSelectionPicker`'s step).
  useDismissOnOutside(open, panelRef, anchor, onClose, () => topPanelOverlay()?.id === idRef.current);

  // Escape via the central overlay stack (not a per-instance keydown listener —
  // those raced each other and the global dispatcher). Keep the latest onClose
  // in a ref so the registered handler always calls the current one without
  // re-registering every render.
  const onCloseRef = useRef(onClose);
  onCloseRef.current = onClose;
  // `useLayoutEffect`, matching the dismiss listeners above rather than landing
  // a frame after them. Those install pre-paint on purpose, so the contract is
  // live from frame zero, and the gate above reads this registration. Split
  // across the two phases, an overlay spent its first frame with live listeners
  // answering nothing. That is the gap the layout effect exists to close. The
  // inert-behind rides along for the same reason.
  useLayoutEffect(() => {
    if (!open) return;
    const id = idRef.current!;
    pushOverlay({ id, dismiss: () => onCloseRef.current(), hasPanel: true });
    acquireOverlayInert();
    return () => {
      removeOverlay(id);
      releaseOverlayInert();
    };
  }, [open]);

  // Keep the anchor toggle interactive while the inert-behind is active. Separate
  // from the effect above (keyed on `[open]`) because the anchor can resolve a
  // render late (e.g. `menuRef.current` is null until the wrapper mounts) — this
  // re-marks the new node if the anchor changes while open, and always unmarks
  // the exact node it marked.
  //
  // `useLayoutEffect` because it is the OTHER half of the inert-behind above,
  // which is pre-paint. Left post-paint, the first painted frame inerts the
  // shell with the anchor not yet exempted from it.
  useLayoutEffect(() => markAnchorInteractive(open ? anchor : null), [open, anchor]);

  if (!open) {
    if (!keepMounted) return null;
    // Mirror the open structure so the hidden element matches what reappears:
    // backdrop mode keeps the `.modal-overlay` container, anchored mode keeps a
    // bare panel. (The full-screen container would add an unwanted backdrop for
    // a no-backdrop overlay.)
    const cls = backdrop
      ? ['modal-overlay', overlayClass, hiddenClass].filter(Boolean).join(' ')
      : [panelClass, hiddenClass].filter(Boolean).join(' ');
    const hidden = <div class={cls} aria-hidden="true" />;
    return usePortal ? createPortal(hidden, document.body) : hidden;
  }

  const panel = (
    <div
      {...panelProps}
      ref={panelRef}
      class={panelClass}
      style={panelStyle}
      role={panelRole}
      aria-modal={ariaModal}
      data-role={dataRole}
      // Stays interactive while `.app-shell` behind it is made inert (see the
      // `data-overlay-open` mechanism above + `global/modal-overlay.css`).
      // Carries this overlay's `overlayStack` id as its VALUE so a panel can be
      // matched back to its stack entry (`dialogKeyScope` asks whether it is the
      // top overlay). Every selector is presence-only (`[data-overlay-panel]`),
      // so the value is free.
      data-overlay-panel={idRef.current}
    >
      {children}
    </div>
  );

  if (!backdrop) return usePortal ? createPortal(panel, document.body) : panel;
  const cls = ['modal-overlay', overlayClass].filter(Boolean).join(' ');
  return <div class={cls}>{panel}</div>;
}
