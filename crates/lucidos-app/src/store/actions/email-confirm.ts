import { activeInlineForm, panelOverlay } from '../store';
import type { EmailConfirmForm } from '../store';
import type { EmailConfirmRequest } from '../types';
import { pushNavState, replaceNavState } from './navigation';
import { revealContentPane } from './pane';

/** Open the email confirmation panel for the draft the engine staged. The panel
 *  takes over the content pane; the user stays on whatever menu item they were
 *  on, so closing it returns them to the view they were actually looking at.
 *  Same shape as `openPluginInstallRequest`.
 *
 *  This deliberately does NOT go through `landOnAccountsWithOverlay` — that
 *  helper belongs to the *credential* request path, which is genuinely about
 *  credentials. Borrowing it here teleported the user to Settings → Accounts to
 *  confirm a send and then stranded them there once the panel closed.
 *
 *  Reveals the content pane: this fires from an engine SSE event, so without it
 *  a mobile user (or a desktop user with a collapsed split) never sees the panel
 *  that just appeared. */
export function openEmailConfirmRequest(request: EmailConfirmRequest): void {
  panelOverlay.value = { type: 'form', form: { type: 'email-confirm', request } };
  pushNavState();
  revealContentPane();
}

/** The send succeeded — turn the open confirm panel into a read-only receipt in
 *  place, instead of closing it and revealing whatever was underneath.
 *
 *  `subject`/`body` are the values that actually went out (the user can edit
 *  them in the form), baked into the request so the receipt still reads
 *  correctly after a remount — a History round-trip or a reload re-seeds the
 *  panel from the form alone.
 *
 *  `replaceNavState`, not `pushNavState`: the panel is already on screen and
 *  mutating in place, so one send keeps one History row, whose label flips to
 *  "Email Sent" via `getFormTitle`. That also retires the stale pending entry —
 *  walking Forward onto it used to re-render a live Send button for an email
 *  that had already gone out.
 *
 *  Returns false when `form` is no longer the active overlay. A send can take up
 *  to ~120s and Escape still dismisses the panel, so a late success must not
 *  resurrect it over whatever the user opened since; the caller falls back to a
 *  toast in that case. Identity comparison, not a type check: a second staged
 *  email would also be an `email-confirm` form, and must not absorb this one's
 *  receipt. */
export function markEmailSent(
  form: EmailConfirmForm,
  sent: { subject: string; body: string },
): boolean {
  if (activeInlineForm.value !== form) return false;
  // Already a receipt — re-stamping would move the timestamp off the real send.
  // Unreachable through the panel (the receipt has no Send button), kept so the
  // marker can only ever be written once.
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
