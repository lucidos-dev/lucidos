// @vitest-environment jsdom
/**
 * The Service Name a user typed survives every re-render of the form.
 *
 * The field was a controlled input with no input handler. Any state change
 * (adding a host, typing a base URL) re-rendered it and restored the empty
 * initial value, so the name vanished mid-edit.
 */
import { describe, it, expect, beforeEach, afterEach, vi } from 'vitest';
import { render } from 'preact';

vi.mock('../../../store/actions/oauth', () => ({
  loadKnownOAuthProviders: vi.fn(async () => {}),
}));

import { CredentialModal } from '../CredentialModal';
import { knownOAuthProviders, panelOverlay } from '../../../store/store';

let host: HTMLElement;

function serviceField(): HTMLInputElement {
  const el = host.querySelector<HTMLInputElement>('input[placeholder="e.g. GitHub, Jira"]');
  if (!el) throw new Error('the service name field is not rendered');
  return el;
}

function button(label: string): HTMLButtonElement {
  const el = [...host.querySelectorAll<HTMLButtonElement>('button')].find(
    (b) => b.textContent === label,
  );
  if (!el) throw new Error(`no "${label}" button`);
  return el;
}

function typeInto(el: HTMLInputElement, value: string): void {
  el.value = value;
  el.dispatchEvent(new Event('input', { bubbles: true }));
}

async function settle(): Promise<void> {
  await new Promise((resolve) => setTimeout(resolve, 0));
}

describe('the credential form keeps the typed service name', () => {
  beforeEach(() => {
    document.body.innerHTML = '';
    host = document.createElement('div');
    document.body.appendChild(host);
    knownOAuthProviders.value = {
      status: 'loaded',
      data: { providers: [], default_redirect_uri: '' },
    };
    panelOverlay.value = { type: 'form', form: { type: 'credential' } };
    render(<CredentialModal />, host);
  });

  afterEach(() => {
    render(null, host);
    document.body.innerHTML = '';
    panelOverlay.value = null;
    knownOAuthProviders.value = { status: 'not-loaded' };
  });

  it('after adding a host', async () => {
    typeInto(serviceField(), 'Example API');
    await settle();

    button('Add another host').click();
    await settle();

    expect(host.querySelectorAll('input[type="url"]')).toHaveLength(2);
    expect(serviceField().value).toBe('Example API');
  });

  it('after typing a base URL', async () => {
    typeInto(serviceField(), 'Example API');
    await settle();

    typeInto(host.querySelector<HTMLInputElement>('input[type="url"]')!, 'https://api.example.com');
    await settle();

    expect(serviceField().value).toBe('Example API');
  });

  // A typed field no longer follows its default, so a request replacing the
  // open form must remount it. Otherwise the save lands under the typed name.
  it('shows the service of a request that replaces a half-filled form', async () => {
    typeInto(serviceField(), 'Example API');
    await settle();

    panelOverlay.value = { type: 'form', form: { type: 'credential', request: { service: 'requested-api' } } };
    await settle();

    expect(serviceField().value).toBe('requested-api');
  });
});
