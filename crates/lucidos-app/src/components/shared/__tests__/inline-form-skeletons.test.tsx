// @vitest-environment jsdom
/** The inline edit forms draw their own frame while their data loads: the
 *  field labels they know as real text, and each control's box shimmering. */
import { afterEach, beforeEach, describe, expect, it, vi } from 'vitest';
import { render, type VNode } from 'preact';
import { act } from 'preact/test-utils';

vi.mock('../../../api/client', async (importOriginal) => ({
  ...(await importOriginal<typeof import('../../../api/client')>()),
  getCredentialValue: () => new Promise(() => {}),
  readAppSourceApi: () => new Promise(() => {}),
}));

import { CredentialModal } from '../../credentials/CredentialModal';
import { AppUiEditModal } from '../../apps/AppUiEditModal';
import { TriggerDetails } from '../../triggers/TriggerDetails';
import { appsList, credentials, panelOverlay, triggers } from '../../../store/store';
import { SPINNER_DELAY_MS } from '../../../hooks/useDelayedLoading';

let host: HTMLDivElement;

function show(node: VNode) {
  act(() => { render(node, host); });
}

function passGate() {
  act(() => { vi.advanceTimersByTime(SPINNER_DELAY_MS); });
}

const labels = () => [...host.querySelectorAll('.loading-fade-skeleton .form-group > label')].map((l) => l.textContent);

beforeEach(() => {
  vi.useFakeTimers({ toFake: ['setTimeout', 'clearTimeout'] });
  host = document.createElement('div');
  document.body.appendChild(host);
});

afterEach(() => {
  render(null, host);
  host.remove();
  panelOverlay.value = null;
  appsList.value = { status: 'not-loaded' };
  credentials.value = { status: 'not-loaded' };
  triggers.value = { status: 'not-loaded' };
  vi.useRealTimers();
});

describe('inline form skeletons', () => {
  it('draws the credential form frame while its secret loads', () => {
    credentials.value = { status: 'loaded', data: [{ id: 'k1', service_name: 'github', auth_type: 'api_key' }] as never };
    panelOverlay.value = { type: 'form', form: { type: 'credential', editing: 'k1' } } as never;
    show(<CredentialModal />);
    expect(host.querySelector('.sk-bar')).toBeNull();
    passGate();
    expect(labels().slice(0, 3)).toEqual(['Service Name', 'Base URLs', 'Auth Type']);
    expect(host.querySelector('.loading-spinner')).toBeNull();
  });

  it('draws the app edit form frame while the app list loads', () => {
    appsList.value = { status: 'loading' };
    panelOverlay.value = { type: 'form', form: { type: 'app-edit', appId: 'a1' } } as never;
    show(<AppUiEditModal />);
    passGate();
    expect(labels().slice(0, 2)).toEqual(['Name', 'Description']);
  });

  it('draws the trigger edit form frame while the trigger list loads', () => {
    triggers.value = { status: 'loading' };
    panelOverlay.value = { type: 'form', form: { type: 'trigger', triggerId: 't1' } } as never;
    show(<TriggerDetails />);
    passGate();
    expect(labels()).toEqual(['Trigger Name', 'Trigger Type', 'Run', 'Intent']);
  });
});
