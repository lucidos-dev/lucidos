import { toasts, dismissToast } from '../../store/store';
import type { ToastType } from '../../store/types';
import { CloseIcon } from './icons';
import { parseToastMessage } from './toastMessage';

const icons: Record<ToastType, string> = {
  success: '<circle cx="12" cy="12" r="10" fill="none" stroke="currentColor" stroke-width="2"/><path d="M8 12l2.5 2.5L16 9" fill="none" stroke="currentColor" stroke-width="2" stroke-linecap="round" stroke-linejoin="round"/>',
  info: '<circle cx="12" cy="12" r="10" fill="none" stroke="currentColor" stroke-width="2"/><line x1="12" y1="16" x2="12" y2="12" stroke="currentColor" stroke-width="2" stroke-linecap="round"/><circle cx="12" cy="8" r="1" fill="currentColor"/>',
  warning: '<path d="M12 2L1 21h22L12 2z" fill="none" stroke="currentColor" stroke-width="2" stroke-linejoin="round"/><line x1="12" y1="14" x2="12" y2="10" stroke="currentColor" stroke-width="2" stroke-linecap="round"/><circle cx="12" cy="17" r="1" fill="currentColor"/>',
  error: '<circle cx="12" cy="12" r="10" fill="none" stroke="currentColor" stroke-width="2"/><line x1="15" y1="9" x2="9" y2="15" stroke="currentColor" stroke-width="2" stroke-linecap="round"/><line x1="9" y1="9" x2="15" y2="15" stroke="currentColor" stroke-width="2" stroke-linecap="round"/>',
};

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
  if (items.length === 0) return null;

  return (
    <div class="toast-container">
      {items.map((t) => (
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
          {t.action && (
            <div class="toast-actions">
              <button class="action-btn" onClick={t.action.onClick}>{t.action.label}</button>
            </div>
          )}
          {(t.type === 'error' || t.type === 'warning' || t.key || t.action || t.onClick) && (
            <button class="icon-btn toast-close" onClick={() => dismissToast(t.key ?? t.id)} aria-label="Dismiss">
              <CloseIcon />
            </button>
          )}
        </div>
      ))}
    </div>
  );
}
