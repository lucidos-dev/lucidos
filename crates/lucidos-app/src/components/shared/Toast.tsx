import { useRef, useLayoutEffect } from 'preact/hooks';
import { toasts, dismissToast, focusedPane, speedMultiplier } from '../../store/store';
import type { ToastType } from '../../store/types';
import { CloseIcon } from './icons';
import { parseToastMessage } from './toastMessage';
import { linkifyText } from './linkifyText';
import { toastAutofocusTarget, toastTabTarget } from './toastFocus';
import { computeToastShifts } from './toastReflow';
import { focusPaneMainControl } from '../layout/paneFocus';
import { hasHoverPointer, prefersReducedMotion } from '../../utils/platform';

// Reflow (FLIP) timing for the toast stack. When a toast is added/removed the
// survivors are re-laid-out instantly by the flex column; these values play that
// displacement back so they glide instead of jumping. Mirrors the `toast-in`
// entry animation in components.css so an incoming toast and the push-down it
// causes move in lockstep — keep the two in sync.
const REFLOW_DURATION_MS = 260;
const REFLOW_EASING = 'cubic-bezier(0.22, 0, 0, 1)';

const icons: Record<ToastType, string> = {
  success: '<circle cx="12" cy="12" r="10" fill="none" stroke="currentColor" stroke-width="2"/><path d="M8 12l2.5 2.5L16 9" fill="none" stroke="currentColor" stroke-width="2" stroke-linecap="round" stroke-linejoin="round"/>',
  info: '<circle cx="12" cy="12" r="10" fill="none" stroke="currentColor" stroke-width="2"/><line x1="12" y1="16" x2="12" y2="12" stroke="currentColor" stroke-width="2" stroke-linecap="round"/><circle cx="12" cy="8" r="1" fill="currentColor"/>',
  warning: '<path d="M12 2L1 21h22L12 2z" fill="none" stroke="currentColor" stroke-width="2" stroke-linejoin="round"/><line x1="12" y1="14" x2="12" y2="10" stroke="currentColor" stroke-width="2" stroke-linecap="round"/><circle cx="12" cy="17" r="1" fill="currentColor"/>',
  error: '<circle cx="12" cy="12" r="10" fill="none" stroke="currentColor" stroke-width="2"/><line x1="15" y1="9" x2="9" y2="15" stroke="currentColor" stroke-width="2" stroke-linecap="round"/><line x1="9" y1="9" x2="15" y2="15" stroke="currentColor" stroke-width="2" stroke-linecap="round"/>',
};

// Toast ids whose default button has already been auto-focused, so a re-render
// (any toast added/removed/updated) never re-steals focus from where the user
// has since moved it. Pruned to the live toasts on each render so it can't grow
// unbounded over a long session.
const autofocusedToastIds = new Set<number>();

/** Ref callback: focus an action toast's default button the first time it
 *  mounts so Enter acts on it immediately. The ref identity churns per render —
 *  the `Set` guard makes the focus fire exactly once per toast. Skips when a
 *  modal/overlay is open (it owns focus while up) — the button stays Tab-able.
 *
 *  Touch devices are skipped entirely (`hasHoverPointer`): there's no keyboard
 *  to press Enter with, so the autofocus buys nothing and only leaves a stray
 *  focus ring on the button (iOS WebKit paints `:focus-visible` for programmatic
 *  focus, which the `hover: hover`-gated toast ring rule can't suppress). Skipped
 *  before the `Set` is marked so the device never records a no-op autofocus. */
function autofocusToastButton(id: number, el: HTMLButtonElement | null): void {
  if (!el || autofocusedToastIds.has(id) || !hasHoverPointer()) return;
  autofocusedToastIds.add(id);
  if (document.documentElement.hasAttribute('data-overlay-open')) return;
  el.focus({ preventScroll: true });
}

/** Keyboard handling for whichever toast currently holds focus (the listener
 *  lives on the container; keydowns bubble up from the focused button):
 *   - Tab cycles forward through that toast's controls (its action buttons, the
 *     close X, and any linkified URL), wrapping off the last back to the first —
 *     so focus stays on the toast the user is acting on.
 *   - Shift+Tab releases focus back to the FOCUSED PANE (the single keyboard
 *     hatch out of the toast), where the per-pane Tab trap then cycles the pane's
 *     own elements. The toast drops out of the tab cycle once left.
 *   - Escape dismisses it (the other hatch out); a non-dismissable toast has no
 *     close button, so Escape is a no-op there.
 *
 *  Both Tab directions `stopPropagation` so the document-level per-pane Tab trap
 *  (`handlePaneTab`) never also fires — without that, a mid-toast forward Tab
 *  would fall through and yank focus into the pane. */
function handleToastKeyDown(e: KeyboardEvent): void {
  const active = document.activeElement as HTMLElement | null;
  const toastEl = active?.closest<HTMLElement>('.toast');
  if (!toastEl) return;

  if (e.key === 'Escape') {
    const closeBtn = toastEl.querySelector<HTMLButtonElement>('.toast-close');
    if (closeBtn) {
      e.preventDefault();
      closeBtn.click();
    }
    return;
  }

  if (e.key !== 'Tab') return;
  // The Tab trap only engages for a toast that owns a button (an action or
  // dismissable toast — Escape via its close button is the hatch out). A
  // link-only toast (e.g. `dismissable: false` with just a URL) must NOT trap:
  // it has no close button, so trapping would strand keyboard focus with no way
  // out. When a button IS present, include anchors so a linkified URL alongside
  // the buttons is reachable by keyboard instead of being skipped by the cycle.
  const buttons = toastEl.querySelectorAll<HTMLButtonElement>('button:not([disabled])');
  if (buttons.length === 0) return;
  const focusables = Array.from(
    toastEl.querySelectorAll<HTMLElement>('a[href], button:not([disabled])'),
  );
  const overlayOpen = document.documentElement.hasAttribute('data-overlay-open');
  const target = toastTabTarget(
    focusables.length,
    focusables.indexOf(active as HTMLElement),
    e.shiftKey,
    overlayOpen,
  );
  if (target === null) return;
  e.preventDefault();
  e.stopPropagation();
  if (target === 'exit') {
    // Shift+Tab: hand focus back to the pane the user was working in. Forceful
    // pane focus lands the pane's scroll surface (or its prompt/first control),
    // from where the per-pane Tab trap takes over. Only reached with no overlay
    // open — `toastTabTarget` keeps focus contained in the toast otherwise, so
    // the forceful pane focus never yanks focus behind an active overlay.
    focusPaneMainControl(focusedPane.value);
  } else {
    focusables[target].focus({ preventScroll: true });
  }
}

function renderMessage(message: string) {
  if (!message.includes('\n')) return linkifyText(message);
  const { heading, sections } = parseToastMessage(message);
  return (
    <>
      {linkifyText(heading)}
      {sections.map((s, i) => (
        <div key={i} class="toast-section">
          {s.title && <div class="toast-section-title">{linkifyText(s.title)}</div>}
          {s.bullets.length > 0 && (
            <ul class="toast-bullets">
              {s.bullets.map((b, j) => <li key={j}>{linkifyText(b)}</li>)}
            </ul>
          )}
        </div>
      ))}
    </>
  );
}

/** Exported wrapper: owns the hooks that drive the toast-stack reflow (FLIP)
 *  animation and delegates rendering to the hook-free `ToastList` below.
 *
 *  The split is deliberate — `ToastList` is a pure function of `toasts.value`
 *  that unit tests invoke directly (`ToastList()`), which is only safe because
 *  it holds no hooks. All the render-context-bound machinery lives here.
 *
 *  Reflow: capture each still-mounted toast's OLD top from the previous render's
 *  DOM (this runs before Preact commits the new children), then in a layout
 *  effect read the NEW tops and animate the delta back to zero. So when a toast
 *  is inserted at the top the survivors glide down instead of snapping, and when
 *  one is removed they glide up to fill the gap. Reads are cheap (a handful of
 *  toasts) and only happen when `toasts` actually changes — nothing else
 *  reactive is read here, so it re-renders only then. */
export function Toast() {
  const items = toasts.value;
  const containerRef = useRef<HTMLDivElement>(null);
  const prevIds = useRef<Set<number>>(new Set());
  const runningAnims = useRef<Animation[]>([]);

  const oldTops = new Map<number, number>();
  const container = containerRef.current;
  if (container) {
    for (const el of container.querySelectorAll<HTMLElement>('[data-toast-id]')) {
      oldTops.set(Number(el.dataset.toastId), el.getBoundingClientRect().top);
    }
  }

  useLayoutEffect(() => {
    const el = containerRef.current;
    const before = prevIds.current;
    prevIds.current = new Set(items.map((t) => t.id));
    if (!el || prefersReducedMotion()) return;

    // A rapid burst of toasts can start a new reflow before the last settled —
    // cancel the in-flight ones so a survivor doesn't fight two transforms.
    for (const a of runningAnims.current) {
      try { a.cancel(); } catch { /* already finished */ }
    }
    runningAnims.current = [];

    const rows = new Map<number, HTMLElement>();
    const current: { id: number; top: number }[] = [];
    for (const row of el.querySelectorAll<HTMLElement>('[data-toast-id]')) {
      const id = Number(row.dataset.toastId);
      rows.set(id, row);
      current.push({ id, top: row.getBoundingClientRect().top });
    }

    const durationMs = REFLOW_DURATION_MS / speedMultiplier.value;
    for (const { id, delta } of computeToastShifts(before, oldTops, current)) {
      const row = rows.get(id);
      if (!row) continue;
      const anim = row.animate(
        [{ transform: `translateY(${delta}px)` }, { transform: 'translateY(0)' }],
        { duration: durationMs, easing: REFLOW_EASING, fill: 'none' },
      );
      runningAnims.current.push(anim);
    }
  });

  return <ToastList containerRef={containerRef} />;
}

/** Pure presentational render of the current toast stack. Holds NO hooks so it
 *  can be called directly in unit tests. `containerRef` is optional — the live
 *  app passes the reflow container ref from `Toast`; tests omit it. */
export function ToastList({ containerRef }: { containerRef?: { current: HTMLDivElement | null } } = {}) {
  const items = toasts.value;

  // Keep the auto-focus memory bounded to the toasts currently on screen.
  if (autofocusedToastIds.size > 0) {
    const live = new Set(items.map((t) => t.id));
    for (const id of autofocusedToastIds) if (!live.has(id)) autofocusedToastIds.delete(id);
  }

  if (items.length === 0) return null;

  // Scale the CSS `toast-in` entry animation by the SAME multiplier the JS
  // reflow uses (the diagnostic animation-speed slider), so an incoming toast
  // and the push-down it causes stay in lockstep at every setting — not just at
  // 1x. The CSS keeps 0.26s as the 1x default; this inline duration overrides it
  // when the slider is off-centre.
  const entryDurationMs = REFLOW_DURATION_MS / speedMultiplier.value;

  return (
    <div ref={containerRef} class="toast-container" onKeyDown={handleToastKeyDown}>
      {items.map((t) => {
        const autoTarget = toastAutofocusTarget(t);
        return (
          <div key={t.id} class={`toast toast-${t.type}`} style={{ animationDuration: `${entryDurationMs}ms` }} data-toast-id={t.id} data-toast-pane={t.pane}>
            <div class="toast-body">
              {t.spinning
                ? <span class="mini-spinner toast-icon" />
                : <svg class="toast-icon" viewBox="0 0 24 24" dangerouslySetInnerHTML={{ __html: icons[t.type] }} />
              }
              <span
                class={`toast-message${t.onClick ? ' toast-clickable' : ''}`}
                onClick={t.onClick}
              >{renderMessage(t.message)}</span>
            </div>
            {(t.action || t.secondaryAction) && (
              <div class="toast-actions">
                {t.secondaryAction && (
                  <button
                    class={`action-btn${t.secondaryAction.variant ? ' action-btn-' + t.secondaryAction.variant : ''}`}
                    ref={autoTarget === 'secondary' ? (el) => autofocusToastButton(t.id, el) : undefined}
                    onClick={t.secondaryAction.onClick}
                  >{t.secondaryAction.label}</button>
                )}
                {t.action && (
                  <button
                    class={`action-btn${t.action.variant ? ' action-btn-' + t.action.variant : ''}`}
                    ref={autoTarget === 'primary' ? (el) => autofocusToastButton(t.id, el) : undefined}
                    onClick={t.action.onClick}
                  >{t.action.label}</button>
                )}
              </div>
            )}
            {t.dismissable !== false && (
              <button
                class="icon-btn toast-close"
                ref={autoTarget === 'close' ? (el) => autofocusToastButton(t.id, el) : undefined}
                onClick={() => dismissToast(t.key ?? t.id)}
                aria-label="Dismiss"
              >
                <CloseIcon />
              </button>
            )}
          </div>
        );
      })}
    </div>
  );
}
