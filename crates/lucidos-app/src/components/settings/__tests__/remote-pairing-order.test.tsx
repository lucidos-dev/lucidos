// @vitest-environment jsdom

import { afterEach, describe, expect, it, vi } from 'vitest';
import { render } from 'preact';
import { useState } from 'preact/hooks';
import { act } from 'preact/test-utils';
import { RemoteGatewaysSection } from '../GatewayPairingSections';
import { pickerFooter, type PickerFooterProps } from '../../picker/PickerFooter';
import { EMPTY_RESTORE_DRAFT } from '../../picker/workspaceForms';
import { HTTPS_ONLY_NOTE } from '../remoteGatewayForm';

vi.mock('../../../api/client/control', async (importOriginal) => ({
  ...(await importOriginal<typeof import('../../../api/client/control')>()),
  listRemoteGateways: vi.fn().mockResolvedValue([]),
}));

const noop = () => {};
const pickerProps: PickerFooterProps = {
  mode: 'remote', onMode: noop, busy: false, restoreRunning: false,
  name: '', onName: noop, onCreate: noop, onCancelCreate: noop,
  suggestions: [], onSuggestion: noop, createNote: null,
  draft: EMPTY_RESTORE_DRAFT, onDraft: noop, onPickFile: noop,
  onRestore: noop, onCancelRestore: noop, blocker: null, fileNote: null,
  onDeleteColliding: noop,
  remoteDraft: { name: 'cloud', code: 'lgp_secret', address: 'https://cloud.example.com' },
  onRemoteDraft: noop, onAddRemote: noop, onCancelRemote: noop,
  pairedRemote: null, remoteAwaitingOwner: false,
};

function PickerForm() {
  const [remoteDraft, setDraft] = useState(pickerProps.remoteDraft);
  return pickerFooter({
    ...pickerProps, remoteDraft, pairedRemote: 'cloud', remoteAwaitingOwner: true,
    onRemoteDraft: patch => setDraft(draft => ({ ...draft, ...patch })),
  });
}

let host: HTMLDivElement;
afterEach(() => {
  render(null, host);
  host.remove();
});

for (const surface of ['Settings', 'picker'] as const) {
  describe(`${surface} remote pairing order`, () => {
    async function mount() {
      host = document.createElement('div');
      document.body.append(host);
      await act(async () => {
        render(surface === 'Settings'
          ? <RemoteGatewaysSection available />
          : <PickerForm />, host);
      });
    }

    function controls() {
      const name = host.querySelector<HTMLInputElement>('input[placeholder="cloud"]')!;
      const all = [...host.querySelectorAll<HTMLInputElement | HTMLButtonElement>('input, button')];
      return all.slice(all.indexOf(name));
    }

    function label(el: HTMLInputElement | HTMLButtonElement) {
      return el instanceof HTMLInputElement ? el.placeholder : el.textContent;
    }

    it('puts fields in native tab order and Pair after Address', async () => {
      await mount();
      const ordered = controls();
      expect(ordered.map(label)).toEqual([
        'cloud', 'lgp_…', 'Extra request headers', 'https://lucidos.example.com', 'Pair',
        ...(surface === 'picker' ? ['Cancel'] : []),
      ]);
      expect(ordered.every(el => el.tabIndex === 0)).toBe(true);
      if (surface === 'Settings') {
        expect(ordered[3].parentElement).toBe(ordered[4].parentElement);
        expect((ordered[4] as HTMLButtonElement).disabled).toBe(true);
      }
    });

    it('keeps expanded header inputs between the secret and Address', async () => {
      await mount();
      await act(() => { controls()[2].click(); });
      const add = controls().find(el => el.textContent === 'Add header')!;
      await act(() => { add.click(); });
      const ordered = controls();
      expect(ordered.map(el => el.getAttribute('aria-label') ?? label(el))).toEqual([
        ...(surface === 'Settings' ? ['Short name', 'Gateway pairing secret'] : ['cloud', 'lgp_…']),
        'Extra request headers', 'Header 1 name', 'Header 1 value', 'Remove header', 'Add header',
        surface === 'Settings' ? 'Address' : 'https://lucidos.example.com', 'Pair',
        ...(surface === 'picker' ? ['Cancel'] : []),
      ]);
      expect(ordered.every(el => el.tabIndex === 0)).toBe(true);
    });

    it('places the HTTPS warning directly after Address', async () => {
      await mount();
      const address = host.querySelector<HTMLInputElement>('input[placeholder="https://lucidos.example.com"]')!;
      await act(() => {
        address.value = 'http://cloud.example.com';
        address.dispatchEvent(new Event('input', { bubbles: true }));
      });
      const row = surface === 'Settings' ? address.closest('.settings-row') : address.closest('label');
      expect(row?.nextElementSibling?.textContent).toBe(HTTPS_ONLY_NOTE);
    });
  });
}
