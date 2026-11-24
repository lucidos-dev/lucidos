
export const COPY_ID_NONCE = ((): string => {
  const bytes = new Uint8Array(8);
  if (typeof crypto !== 'undefined' && typeof crypto.getRandomValues === 'function') {
    crypto.getRandomValues(bytes);
  } else {
    for (let i = 0; i < bytes.length; i++) bytes[i] = Math.floor(Math.random() * 256);
  }
  return Array.from(bytes, (b) => b.toString(16).padStart(2, '0')).join('');
})();

export const CODE_COPY_ATTR = 'data-code-copy';

export const TASK_CHECKBOX_ATTR = 'data-task-checkbox';

export const taskCheckboxTag = (checked: boolean): string =>
  `<input ${TASK_CHECKBOX_ATTR}="${COPY_ID_NONCE}"${checked ? ' checked' : ''} disabled type="checkbox">`;

export const BARE_EMAIL_ATTR = 'data-bare-email';

export const WIDGET_EMBED_SLOT_ATTR = 'data-widget-embed-slot';
export const WIDGET_EMBED_ATTR = 'data-widget-embed';
export const WIDGET_EMBED_ERROR_ATTR = 'data-widget-embed-error';
