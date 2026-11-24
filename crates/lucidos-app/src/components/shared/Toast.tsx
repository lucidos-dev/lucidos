import { useRef, useLayoutEffect } from 'preact/hooks';
import { toasts, dismissToast, focusedPane, scaledDurationMs } from '../../store/store';
import type { ToastAction, ToastItem } from '../../store/types';
import { CloseIcon, ToneIcon } from './icons';
import { linkifyText } from './linkifyText';
import { toastHasClose, toastTabTarget } from './toastFocus';
import { toastTap } from './toastTap';
import { computeToastShifts } from './toastReflow';
import { toastStackUrgency } from './toastUrgency';
import { focusPaneMainControl, visibleFocusables } from '../layout/paneFocus';
import { topPanelOverlay } from '../../store/overlayStack';
import { isReducedMotion } from '../../utils/motion';
import { hasHoverPointer } from '../../utils/platform';
import { progressFillWidth } from './progressBar';

const REFLOW_DURATION_MS = 260;
const REFLOW_EASING = 'cubic-bezier(0.22, 0, 0, 1)';

export const INLINE_MESSAGE_MAX_CHARS = 60;

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
    focusPaneMainControl(focusedPane.value);
  } else {
    focusables[target].focus({ preventScroll: true });
  }
}

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

export function Toast() {
  const items = toasts.value;
  const containerRef = useRef<HTMLDivElement>(null);
  const prevIds = useRef<Set<number>>(new Set());
  const runningAnims = useRef<Animation[]>([]);

  const oldTops = new Map<number, number>();
  const container = containerRef.current;
  const active = document.activeElement;
  const focusedControl = active && container?.contains(active) ? active : null;
  if (container) {
    for (const el of container.querySelectorAll<HTMLElement>('[data-toast-id]')) {
      oldTops.set(Number(el.dataset.toastId), el.getBoundingClientRect().top);
    }
  }

  useLayoutEffect(() => {
    if (hasHoverPointer() && focusedControl && !focusedControl.isConnected && document.activeElement === document.body) {
      focusNewestToast();
      if (document.activeElement === document.body) {
        const overlay = topPanelOverlay();
        const panel = overlay && document.querySelector<HTMLElement>(`[data-overlay-panel="${overlay.id}"]`);
        if (panel) (visibleFocusables(panel)[0] ?? panel).focus({ preventScroll: true });
        else if (!overlay) focusPaneMainControl(focusedPane.value);
      }
    }
    const el = containerRef.current;
    const before = prevIds.current;
    prevIds.current = new Set(items.map((t) => t.id));
    if (!el || isReducedMotion()) return;

    for (const a of runningAnims.current) {
      try { a.cancel(); } catch {  }
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

export function ToastList({ containerRef }: { containerRef?: { current: HTMLDivElement | null } } = {}) {
  const items = toasts.value;

  if (items.length === 0) return null;

  const entryDurationMs = scaledDurationMs(REFLOW_DURATION_MS);

  return (
    <div
      ref={containerRef}
      class="toast-container"
      data-toast-urgency={toastStackUrgency(items)}
      onKeyDown={handleToastKeyDown}
    >
      <div class="toast-column">
        {items.map((t) => renderToast(t, entryDurationMs))}
      </div>
    </div>
  );
}

function renderToast(t: ToastItem, entryDurationMs: number) {
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
  const onCardClick = tapAction
    ? (e: MouseEvent) => {
        if ((e.target as HTMLElement | null)?.closest('a[href], button')) return;
        const selection = window.getSelection();
        const card = e.currentTarget as Node;
        if (selection && !selection.isCollapsed && card.contains(selection.anchorNode)) return;
        tapAction();
      }
    : undefined;
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
      {t.spinning
        ? <span class="mini-spinner toast-icon" />
        : <span class="toast-icon" aria-hidden="true"><ToneIcon tone={t.type} /></span>
      }
      <div class="toast-body">
        <div class="toast-heading" tabIndex={-1}>
          {t.title ? <span class="toast-title">{linkifyText(t.title)}</span> : linkifyText(t.message)}
          {t.count && (
            <span class="label label-neutral toast-count">×{t.count}</span>
          )}
        </div>
        {t.title && <div class="toast-text" tabIndex={-1}>{linkifyText(t.message)}</div>}
      </div>
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
