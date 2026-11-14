// @vitest-environment jsdom
/**
 * A confirm's OK button is red only when the caller says the action destroys
 * something. `tsc` requires the variant at every call site, and this test pins
 * what each variant renders.
 */
import { describe, it, expect, beforeEach, afterEach } from 'vitest';
import { render } from 'preact';

import { ConfirmDialog } from '../ConfirmDialog';
import { confirmState, showConfirm } from '../../../store/store';
import { _resetOverlayStackForTesting } from '../../../store/overlayStack';

function settled(): Promise<void> {
  return new Promise((resolve) => setTimeout(resolve, 0));
}

describe('the confirm dialog paints its OK by the caller\'s variant', () => {
  let host: HTMLDivElement;

  beforeEach(() => {
    _resetOverlayStackForTesting();
    confirmState.value = { visible: false, message: '', okLabel: '' };
    host = document.createElement('div');
    document.body.appendChild(host);
    render(<ConfirmDialog />, host);
  });

  afterEach(() => {
    confirmState.peek().resolve?.(false);
    render(null, host);
    host.remove();
  });

  function okButton(): HTMLButtonElement {
    const ok = document.querySelector<HTMLButtonElement>('[data-role="confirm-ok"]');
    expect(ok, 'the dialog renders an OK button').not.toBeNull();
    return ok!;
  }

  it('is red for a destructive action', async () => {
    void showConfirm('Delete the trigger "Daily digest"?', 'Delete', { variant: 'danger' });
    await settled();
    expect(okButton().classList.contains('action-btn-danger')).toBe(true);
  });

  it('is the plain blue action button for everything else', async () => {
    void showConfirm('Archive this thread?', 'Archive', { variant: 'default' });
    await settled();
    const ok = okButton();
    expect(ok.classList.contains('action-btn')).toBe(true);
    expect(ok.classList.contains('action-btn-danger')).toBe(false);
  });

  it('builds on the shared surface, with Cancel as the outlined secondary', async () => {
    void showConfirm('Revert this change?', 'Revert', { variant: 'default', title: 'Revert change' });
    await settled();
    const panel = document.querySelector('.confirm-dialog')!;
    expect(panel.classList.contains('surface')).toBe(true);
    expect(panel.querySelector('.surface-head .surface-title')?.textContent).toBe('Revert change');
    const cancel = panel.querySelector('[data-role="confirm-cancel"]')!;
    expect(cancel.classList.contains('action-btn-secondary')).toBe(true);
  });
});
