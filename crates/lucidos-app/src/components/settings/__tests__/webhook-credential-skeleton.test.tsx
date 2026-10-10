// @vitest-environment jsdom
/** The webhook signature's saved-credential picker wears its own box while the
 *  credential list loads, past the delay gate, never a line of "Loading…". */
import { afterEach, beforeEach, expect, it, vi } from 'vitest';
import { render } from 'preact';
import { act } from 'preact/test-utils';
import { WebhookSignatureFields, newSignatureDraft } from '../WebhookSignatureFields';
import { SPINNER_DELAY_MS } from '../../../hooks/useDelayedLoading';
import type { CredentialInfo, Loadable } from '../../../store/types';

let host: HTMLDivElement;
const draft = { ...newSignatureDraft('deploys'), source: 'saved' as const };

function show(credentials: Loadable<CredentialInfo[]>) {
  act(() => { render(<WebhookSignatureFields draft={draft} credentials={credentials} onChange={() => {}} />, host); });
}

function credentialRow(): Element {
  return [...host.querySelectorAll('.webhook-signature-row')].find((r) => r.textContent?.startsWith('Credential'))!;
}

beforeEach(() => {
  vi.useFakeTimers({ toFake: ['setTimeout', 'clearTimeout'] });
  host = document.createElement('div');
  document.body.appendChild(host);
});

afterEach(() => {
  render(null, host);
  host.remove();
  vi.useRealTimers();
});

it('draws nothing in the slot before the delay gate opens', () => {
  show({ status: 'loading' });
  expect(credentialRow().querySelector('.sk-bar')).toBeNull();
  expect(credentialRow().textContent).toBe('Credential');
});

it('draws the picker box once the gate opens', () => {
  show({ status: 'loading' });
  act(() => { vi.advanceTimersByTime(SPINNER_DELAY_MS); });
  expect(credentialRow().querySelector('.dropdown-skeleton .sk-bar')).not.toBeNull();
  expect(credentialRow().textContent).not.toContain('Loading');
});

it('says none are saved once an empty list lands', () => {
  show({ status: 'loaded', data: [] });
  expect(credentialRow().querySelector('.settings-section-desc')?.textContent).toContain('No saved credentials yet');
});
