import { computed } from '@preact/signals';
import { useRef, useLayoutEffect } from 'preact/hooks';
import { toasts, dismissToast, focusedPane, scaledDurationMs, splitRatio, toastPlacement } from '../../store/store';
import type { ToastAction, ToastItem } from '../../store/types';
import { CloseIcon, ToneIcon } from './icons';
import { linkifyText } from './linkifyText';
import { toastHasClose, toastTabTarget } from './toastFocus';
import { toastTap } from './toastTap';
import { computeToastShifts } from './toastReflow';
import { toastColumns, toastLayout } from './toastColumns';
import { toastStackUrgency } from './toastUrgency';
import { focusPaneMainControl } from '../layout/paneFocus';
import { isReducedMotion } from '../../utils/motion';
import { viewportIsMobile } from '../../utils/viewport';
import { progressFillWidth } from './progressBar';

// Reflow (FLIP) timing for the toast stack. When a toast is added/removed the
// survivors are re-laid-out instantly by the flex column; these values play that
// displacement back so they glide instead of jumping. Mirrors the `toast-in`
// entry animation in components.css so an incoming toast and the push-down it
// causes move in lockstep — keep the two in sync.
const REFLOW_DURATION_MS = 260;
const REFLOW_EASING = 'cubic-bezier(0.22, 0, 0, 1)';

/** The longest untitled message the one-line layout takes: about what fits
 *  beside the icon and a text action in a desktop toast, at the monospace body
 *  size. */
export const INLINE_MESSAGE_MAX_CHARS = 60;

/** Which per-pane stacks the toast container lays out this frame. Derived
 *  rather than read inline so the toast list re-renders when the LAYOUT flips,
 *  not on every `splitRatio` write: a divider drag writes the ratio on every
 *  pointermove, and a computed only wakes its readers when its value actually
 *  changes. */
const paneLayout = computed(() =>
  toastLayout(viewportIsMobile.value, splitRatio.value, toastPlacement.value),
);

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

/** Move keyboard focus to the first control of the newest toast that has one,
 *  so its Tab trap and Escape take over. Toasts never take focus themselves, so
 *  this shortcut is the keyboard's way in. A timed passive toast has nothing to
 *  act on and is skipped. A no-op when no toast offers a control, and while a
 *  standing stack sits under an open overlay (the rule keyed on
 *  `data-toast-urgency` in components.css): focus must not leave the overlay
 *  for something drawn beneath it. */
export function focusNewestToast(): void {
  const root = document.documentElement;
  const underOverlay = root.hasAttribute('data-overlay-open') && !root.hasAttribute('data-ui-blocked')
    && toastStackUrgency(toasts.value) === 'standing';
  if (underOverlay) return;
  for (const t of toasts.value) {
    const control = document.querySelector<HTMLElement>(
      `.toast[data-toast-id="${t.id}"] :is(a[href], button:not([disabled]))`,
    );
    if (control) {
      control.focus({ preventScroll: true });
      return;
    }
  }
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
    if (!el || isReducedMotion()) return;

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

    const durationMs = scaledDurationMs(REFLOW_DURATION_MS);
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

  if (items.length === 0) return null;

  // Scale the CSS `toast-in` entry animation by the SAME multiplier the JS
  // reflow uses (the diagnostic animation-speed slider), so an incoming toast
  // and the push-down it causes stay in lockstep at every setting — not just at
  // 1x. The CSS keeps 0.26s as the 1x default; this inline duration overrides it
  // when the slider is off-centre.
  const entryDurationMs = scaledDurationMs(REFLOW_DURATION_MS);

  // One stack per VISIBLE pane, so a toast only ever pushes down the toasts of
  // its own pane (`toastColumns`). The pane split is decided here rather than
  // left to CSS because a collapsed pane has to MERGE the two stacks back into
  // one, which no amount of positioning can do to two separate columns.
  const columns = toastColumns(items, paneLayout.value);

  return (
    <div
      ref={containerRef}
      class="toast-container"
      /* The shape under comparison. Every difference between the four is a CSS
         rule keyed on this, so the render stays one code path. Temporary, and it
         goes with the picker (docs/temporary-measures.md). */
      data-toast-placement={toastPlacement.value}
      /* Whether this stack may paint over an open modal (`toastUrgency.ts`).
         The container carries the z-index and is therefore one stacking
         context, so the answer is per stack rather than per toast. */
      data-toast-urgency={toastStackUrgency(items)}
      onKeyDown={handleToastKeyDown}
    >
      {columns.map((column) => (
        <div
          key={column.pane ?? 'single'}
          class="toast-column"
          data-toast-pane={column.pane ?? undefined}
        >
          {column.items.map((t) => renderToast(t, entryDurationMs))}
        </div>
      ))}
    </div>
  );
}

/** One toast row. A plain function rather than a component so the surrounding
 *  `ToastList` stays hook-free and directly callable from unit tests.
 *
 *  The toast is mounted in two boxes, and that split IS the layout. The title
 *  goes in the heading, outside the scroll box. So a scroll to the end of the
 *  message still shows what the toast is about. The text box below can then run
 *  to the card's right edge, where the close X is not. An untitled toast is its
 *  message alone, in the heading. See the `.toast-heading` and `.toast-text`
 *  rules in components.css. */
function renderToast(t: ToastItem, entryDurationMs: number) {
  // A short, single-line, untitled toast lays out on one line, with any action
  // beside the words (`.toast-inline`). A long headline takes the block layout
  // instead: in a row it would be squeezed into a narrow column, and its scroll
  // box would sit mid-card beside the action.
  const inline = !t.title && !t.message.includes('\n') && t.progress == null && !t.secondaryAction
    && t.message.length <= INLINE_MESSAGE_MAX_CHARS;
  const actionClass = (action: ToastAction, fallback?: string) => {
    const variant = action.variant ? `action-btn-${action.variant}` : fallback;
    return variant ? `action-btn ${variant}` : 'action-btn';
  };
  const tap = toastTap(t);
  const tapAction = tap === 'click' ? t.onClick
    : tap === 'action' ? t.action?.onClick
    : undefined;
  // The card takes a tap anywhere, padding and icon included. A tap on a button
  // or a linkified URL stands down, since each is its own destination. So does
  // the mouseup that ends a text selection inside the card. The anchor cannot
  // swallow the click itself: `onGlobalClick` opens it from a document listener.
  const onCardClick = tapAction
    ? (e: MouseEvent) => {
        if ((e.target as HTMLElement | null)?.closest('a[href], button')) return;
        const selection = window.getSelection();
        const card = e.currentTarget as Node;
        if (selection && !selection.isCollapsed && card.contains(selection.anchorNode)) return;
        tapAction();
      }
    : undefined;
  // A card that acts keeps a real button for the keyboard and screen readers,
  // hidden from sight. The card is no `role="button"` itself, which would hide
  // the X and any link inside it from assistive tech.
  const keyLabel = tap === 'action' ? t.action?.label : tap === 'click' ? 'Open' : undefined;
  return (
    <div
      key={t.id}
      class={`toast surface toast-${t.type}${inline ? ' toast-inline' : ''}`}
      style={{ animationDuration: `${entryDurationMs}ms` }}
      data-toast-id={t.id}
      data-toast-tap={tap ?? undefined}
      onClick={onCardClick}
    >
      {/* The icon is the body's SIBLING, positioned over the gutter the heading
          pads out for it. Out of the flow it can never sit inside a scroll box,
          which is what used to shear the spinning one. */}
      {t.spinning
        ? <span class="mini-spinner toast-icon" />
        : <span class="toast-icon" aria-hidden="true"><ToneIcon tone={t.type} /></span>
      }
      <div class="toast-body">
        {/* `tabIndex={-1}` on both scroll boxes, so Chrome leaves them out of
            the Tab order. It promotes an overflowing scroller with no focusable
            child to a Tab stop, and a toast that grew past the cap has two of
            them. `handleToastKeyDown` above is the toast's whole keyboard
            contract and knows only buttons and links, so each promoted box was
            a stop it could not name, wearing the browser's default ring around
            the message text. Losing the stop costs nothing: a click already
            blurred whatever held focus, and the buttons are unaffected. */}
        <div class="toast-heading" tabIndex={-1}>
          {t.title ? <span class="toast-title">{linkifyText(t.title)}</span> : linkifyText(t.message)}
          {t.count && (
            <span class="label label-neutral toast-count">×{t.count}</span>
          )}
        </div>
        {t.title && <div class="toast-text" tabIndex={-1}>{linkifyText(t.message)}</div>}
      </div>
      {/* Determinate progress for a long operation (a packaged update's
          download). Absent when the operation has no honest percentage: the
          spinner and the message carry it instead. `progressFillWidth` clamps,
          so a bad fraction can never paint outside the track. */}
      {t.progress != null && (
        <div class="progress-bar toast-progress">
          <div class="progress-bar-fill" style={{ width: progressFillWidth(t.progress) }} />
        </div>
      )}
      {keyLabel && (
        <button
          class="toast-tap-control visually-hidden"
          onClick={tapAction}
        >{keyLabel}</button>
      )}
      {tap !== 'action' && (t.action || t.secondaryAction) && (
        <div class="toast-actions button-group">
          {t.secondaryAction && (
            <button
              class={actionClass(t.secondaryAction, 'action-btn-secondary')}
              onClick={t.secondaryAction.onClick}
            >{t.secondaryAction.label}</button>
          )}
          {t.action && (
            <button
              class={actionClass(t.action)}
              onClick={t.action.onClick}
            >{t.action.label}</button>
          )}
        </div>
      )}
      {toastHasClose(t) && (
        <button
          class="icon-btn toast-close"
          onClick={() => dismissToast(t.key ?? t.id)}
          aria-label="Dismiss"
          data-tooltip="Dismiss"
        >
          <CloseIcon />
        </button>
      )}
    </div>
  );
}
