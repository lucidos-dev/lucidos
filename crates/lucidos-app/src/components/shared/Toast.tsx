import { toasts, dismissToast } from '../../store/store';
import type { ToastType } from '../../store/types';
import { CloseIcon } from './icons';
import { parseToastMessage } from './toastMessage';
import { toastAutofocusTarget } from './toastFocus';
import { trapTargetIndex } from '../layout/paneFocus';

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
 *  modal/overlay is open (it owns focus while up) — the button stays Tab-able. */
function autofocusToastButton(id: number, el: HTMLButtonElement | null): void {
  if (!el || autofocusedToastIds.has(id)) return;
  autofocusedToastIds.add(id);
  if (document.documentElement.hasAttribute('data-overlay-open')) return;
  el.focus({ preventScroll: true });
}

/** Keyboard handling for whichever toast currently holds focus (the listener
 *  lives on the container; keydowns bubble up from the focused button):
 *   - Tab / Shift+Tab cycle through that toast's buttons (wrap at the ends), so
 *     focus stays on the toast the user is acting on.
 *   - Escape dismisses it (the escape hatch from the auto-focus + Tab cycle);
 *     a non-dismissable toast has no close button, so Escape is a no-op there. */
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
  const btns = Array.from(toastEl.querySelectorAll<HTMLButtonElement>('button:not([disabled])'));
  const target = trapTargetIndex(btns.length, btns.indexOf(active as HTMLButtonElement), e.shiftKey);
  if (target !== null) {
    e.preventDefault();
    btns[target].focus({ preventScroll: true });
  }
}

function renderMessage(message: string) {
  if (!message.includes('\n')) return message;
  const { heading, sections } = parseToastMessage(message);
  return (
    <>
      {heading}
      {sections.map((s, i) => (
        <div key={i} class="toast-section">
          {s.title && <div class="toast-section-title">{s.title}</div>}
          {s.bullets.length > 0 && (
            <ul class="toast-bullets">
              {s.bullets.map((b, j) => <li key={j}>{b}</li>)}
            </ul>
          )}
        </div>
      ))}
    </>
  );
}

export function Toast() {
  const items = toasts.value;

  // Keep the auto-focus memory bounded to the toasts currently on screen.
  if (autofocusedToastIds.size > 0) {
    const live = new Set(items.map((t) => t.id));
    for (const id of autofocusedToastIds) if (!live.has(id)) autofocusedToastIds.delete(id);
  }

  if (items.length === 0) return null;

  return (
    <div class="toast-container" onKeyDown={handleToastKeyDown}>
      {items.map((t) => {
        const autoTarget = toastAutofocusTarget(t);
        return (
          <div key={t.id} class={`toast toast-${t.type}`}>
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
