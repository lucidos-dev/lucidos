import { describe, it, expect, beforeEach, vi } from 'vitest';
import { activeInlineForm, activeMenuItem, panelOverlay, settingsSubview } from '../store';
import type { EmailConfirmRequest } from '../types';

// Spy on the nav-stack writes: a send mutates the panel in place, so it must
// REPLACE the entry it already owns, never push a second one.
const pushNavState = vi.fn();
const replaceNavState = vi.fn();
vi.mock('./navigation', () => ({ pushNavState, replaceNavState }));

// Every user-intent navigation that lands content in the right pane must call
// revealContentPane() — this one fires from an engine SSE event, so without it
// a mobile user never sees the panel that just appeared.
const revealContentPane = vi.fn();
vi.mock('./pane', () => ({ revealContentPane }));

const { openEmailConfirmRequest, markEmailSent } = await import('./email-confirm');

const draft: EmailConfirmRequest = {
  to: ['recipient@example.com'],
  subject: 'Quarterly numbers',
  body: 'Draft body from the engine.',
  cc: ['cc@example.com'],
  account: 'work',
  from: 'me@example.com',
};

function openDraft() {
  openEmailConfirmRequest(draft);
  const form = activeInlineForm.value;
  if (form?.type !== 'email-confirm') throw new Error('expected an email-confirm form');
  return form;
}

beforeEach(() => {
  panelOverlay.value = null;
  activeMenuItem.value = 'files';
  settingsSubview.value = 'main';
  pushNavState.mockClear();
  replaceNavState.mockClear();
  revealContentPane.mockClear();
});

describe('openEmailConfirmRequest', () => {
  it('opens the confirm panel over the current view without navigating anywhere', () => {
    openEmailConfirmRequest(draft);

    expect(panelOverlay.value).toEqual({
      type: 'form',
      form: { type: 'email-confirm', request: draft },
    });
    // The regression this pins: the handler used to call
    // landOnAccountsWithOverlay, which teleported the user to Settings →
    // Accounts to confirm a send and stranded them there once the panel closed.
    expect(activeMenuItem.value).toBe('files');
    expect(settingsSubview.value).toBe('main');
  });

  it('pushes a history entry and reveals the content pane', () => {
    openEmailConfirmRequest(draft);

    expect(pushNavState).toHaveBeenCalledTimes(1);
    expect(revealContentPane).toHaveBeenCalledTimes(1);
  });
});

describe('markEmailSent', () => {
  it('turns the open panel into a receipt carrying the values actually sent', () => {
    const form = openDraft();

    expect(markEmailSent(form, { subject: 'Q3 numbers', body: 'Edited body.' })).toBe(true);

    const sent = activeInlineForm.value;
    expect(sent?.type).toBe('email-confirm');
    if (sent?.type !== 'email-confirm') return;
    // The user can edit subject/body in the form; the receipt (and the history
    // entry behind it) must show what went out, not the engine's draft — a
    // remount re-seeds the panel from the form alone.
    expect(sent.request.subject).toBe('Q3 numbers');
    expect(sent.request.body).toBe('Edited body.');
    expect(sent.request.to).toEqual(draft.to);
    expect(sent.request.cc).toEqual(draft.cc);
    expect(sent.sentAt).toBeTruthy();
  });

  it('replaces the panel history entry instead of pushing a second one', () => {
    const form = openDraft();
    pushNavState.mockClear();

    markEmailSent(form, { subject: draft.subject, body: draft.body });

    expect(replaceNavState).toHaveBeenCalledTimes(1);
    expect(pushNavState).not.toHaveBeenCalled();
  });

  it('no-ops when the user dismissed the panel mid-send', () => {
    const form = openDraft();
    panelOverlay.value = null;

    expect(markEmailSent(form, { subject: draft.subject, body: draft.body })).toBe(false);
    expect(panelOverlay.value).toBeNull();
    expect(replaceNavState).not.toHaveBeenCalled();
  });

  it('no-ops when a different email-confirm panel took over mid-send', () => {
    const form = openDraft();
    // A second staged email is also an `email-confirm` form — a type check would
    // let this one's receipt overwrite it, so the guard compares identity.
    const other = openDraft();
    replaceNavState.mockClear();

    expect(markEmailSent(form, { subject: draft.subject, body: draft.body })).toBe(false);
    expect(activeInlineForm.value).toBe(other);
    expect(replaceNavState).not.toHaveBeenCalled();
  });

  it('never re-stamps an already-sent receipt', () => {
    const form = openDraft();
    markEmailSent(form, { subject: draft.subject, body: draft.body });
    const receipt = activeInlineForm.value;
    if (receipt?.type !== 'email-confirm') throw new Error('expected a receipt');
    replaceNavState.mockClear();

    expect(markEmailSent(receipt, { subject: 'again', body: 'again' })).toBe(false);
    expect(activeInlineForm.value).toBe(receipt);
    expect(replaceNavState).not.toHaveBeenCalled();
  });
});
