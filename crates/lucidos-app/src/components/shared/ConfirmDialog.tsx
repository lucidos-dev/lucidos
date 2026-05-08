import { useEffect, useRef } from 'preact/hooks';
import { confirmState } from '../../store/store';
import { isTauri } from '../../utils/platform';
import { hidePanelWebview, showPanelWebview } from '../../utils/tauri';
import { ModalOverlay } from './ModalOverlay';

function resolve(value: boolean) {
  const state = confirmState.peek();
  state.resolve?.(value);
  confirmState.value = { visible: false, message: '', okLabel: 'Delete' };
}

export function ConfirmDialog() {
  const state = confirmState.value;
  const dialogRef = useRef<HTMLDivElement>(null);
  const okBtnRef = useRef<HTMLButtonElement>(null);

  useEffect(() => {
    if (!state.visible) return;
    if (isTauri()) hidePanelWebview();

    okBtnRef.current?.focus();

    function handleKey(e: KeyboardEvent) {
      const target = e.target as HTMLElement | null;
      if (e.key === 'Enter') {
        // Buttons handle Enter natively (triggers click); textareas need newlines.
        if (target?.tagName === 'BUTTON' || target?.tagName === 'TEXTAREA') return;
        e.preventDefault();
        resolve(true);
        return;
      }
      if (e.key !== 'Tab') return;
      const root = dialogRef.current;
      if (!root) return;
      const focusables = root.querySelectorAll<HTMLElement>(
        'button:not([disabled]), [href], input:not([disabled]), select:not([disabled]), textarea:not([disabled]), [tabindex]:not([tabindex="-1"])'
      );
      if (focusables.length === 0) return;
      const first = focusables[0];
      const last = focusables[focusables.length - 1];
      const active = document.activeElement as HTMLElement | null;
      if (e.shiftKey && active === first) {
        e.preventDefault();
        last.focus();
      } else if (!e.shiftKey && active === last) {
        e.preventDefault();
        first.focus();
      }
    }
    document.addEventListener('keydown', handleKey);
    return () => {
      document.removeEventListener('keydown', handleKey);
      if (isTauri()) showPanelWebview();
    };
  }, [state.visible]);

  if (!state.visible) return null;

  const cancelLabel = state.cancelLabel || 'Cancel';

  return (
    <ModalOverlay onClose={() => resolve(false)}>
      <div ref={dialogRef} class="confirm-dialog" role="dialog" aria-modal="true" onClick={(e) => e.stopPropagation()}>
        {state.title && <h2 class="confirm-title">{state.title}</h2>}
        <p class="confirm-message">{state.message}</p>
        {state.details && (
          <div class="confirm-details">
            {state.details.intro && <p class="confirm-details-intro">{state.details.intro}</p>}
            {state.details.groups.map((g, gi) => (
              <div class="confirm-details-group" key={gi}>
                <div class="confirm-details-header">{g.header}</div>
                {g.items.length > 0 && (
                  <ul class="confirm-details-list">
                    {g.items.map((item, ii) => <li key={ii}>{item}</li>)}
                  </ul>
                )}
              </div>
            ))}
          </div>
        )}
        <div class="confirm-actions">
          {state.extraAction && (
            <button
              class="confirm-btn confirm-btn-cancel confirm-btn-extra"
              onClick={() => {
                state.extraAction!.onClick();
                resolve(false);
              }}
            >
              {state.extraAction.label}
            </button>
          )}
          <div class="confirm-actions-right">
            <button class="confirm-btn confirm-btn-cancel" onClick={() => resolve(false)}>
              {cancelLabel}
            </button>
            <button
              ref={okBtnRef}
              class={`confirm-btn ${state.variant === 'default' ? 'confirm-btn-ok-default' : 'confirm-btn-ok'}`}
              onClick={() => resolve(true)}
            >
              {state.okLabel}
            </button>
          </div>
        </div>
      </div>
    </ModalOverlay>
  );
}
