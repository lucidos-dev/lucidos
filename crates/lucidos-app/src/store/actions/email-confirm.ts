import { activeInlineForm, closeInlineForm, panelOverlay } from '../store';
import type { EmailConfirmForm } from '../store';
import type { EmailConfirmRequest } from '../types';
import { pushNavState, replaceNavState } from './navigation';
import { revealContentPane } from './pane';
import { cancelFormRequestById } from './form-request-cancel';

export function openEmailConfirmRequest(request: EmailConfirmRequest): void {
  panelOverlay.value = { type: 'form', form: { type: 'email-confirm', request } };
  pushNavState();
  revealContentPane();
}

export function markEmailSent(
  form: EmailConfirmForm,
  sent: { subject: string; body: string },
): boolean {
  if (activeInlineForm.value !== form) return false;
  if (form.sentAt) return false;
  panelOverlay.value = {
    type: 'form',
    form: {
      type: 'email-confirm',
      request: { ...form.request, subject: sent.subject, body: sent.body },
      sentAt: new Date().toISOString(),
    },
  };
  replaceNavState();
  return true;
}

export function cancelEmailConfirm(form: EmailConfirmForm): void {
  closeInlineForm();
  const requestId = form.request.form_request_id;
  if (requestId) void cancelFormRequestById(requestId);
}
