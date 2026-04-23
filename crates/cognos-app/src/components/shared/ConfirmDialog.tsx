import { useEffect } from 'preact/hooks';
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

  useEffect(() => {
    if (!state.visible || !isTauri()) return;
    hidePanelWebview();
    return () => showPanelWebview();
  }, [state.visible]);

  if (!state.visible) return null;

  return (
    <ModalOverlay onClose={() => resolve(false)}>
      <div class="confirm-dialog" onClick={(e) => e.stopPropagation()}>
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
              Cancel
            </button>
            <button class={`confirm-btn ${state.variant === 'default' ? 'confirm-btn-ok-default' : 'confirm-btn-ok'}`} onClick={() => resolve(true)}>
              {state.okLabel}
            </button>
          </div>
        </div>
      </div>
    </ModalOverlay>
  );
}
