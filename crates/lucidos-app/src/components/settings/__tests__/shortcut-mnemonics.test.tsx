// @vitest-environment jsdom
import { describe, it, expect, beforeEach } from 'vitest';
import { render } from 'preact';
import { act } from 'preact/test-utils';
import { KeyboardShortcutsSection } from '../KeyboardShortcutsSection';
import { preferences } from '../../../store/store';
import { KEYBINDINGS_PREF_KEY } from '../../../store/actions/keybindings';

let host: HTMLDivElement;

function rowOf(label: string): HTMLElement {
  const title = [...host.querySelectorAll<HTMLElement>('.list-row .title')].find((t) => t.textContent === label);
  expect(title, `no row for ${label}`).toBeDefined();
  return title!.closest<HTMLElement>('.list-row')!;
}

function mnemonicOf(label: string): string | null {
  return rowOf(label).querySelector('[data-role="shortcut-mnemonic"]')?.textContent ?? null;
}

function mount(keybindings?: Record<string, string>): void {
  preferences.value = {
    status: 'loaded',
    data: keybindings ? { [KEYBINDINGS_PREF_KEY]: JSON.stringify(keybindings) } : {},
  };
  act(() => {
    render(<KeyboardShortcutsSection />, host);
  });
}

beforeEach(() => {
  host?.remove();
  host = document.createElement('div');
  document.body.appendChild(host);
});

describe('shortcut mnemonics', () => {
  it('shows the mnemonic under a shortcut on its default chord', () => {
    mount();
    expect(mnemonicOf('Follow the live edge (arm or disarm)')).toBe('L for Live');
  });

  it('hides it once the user rebinds that shortcut', () => {
    mount({ followLiveEdge: 'mod+shift+j' });
    expect(mnemonicOf('Follow the live edge (arm or disarm)')).toBeNull();
    expect(mnemonicOf('Show what the thread changed')).toBe('D for Diff');
  });

  it('shows nothing for a shortcut with no mnemonic', () => {
    mount();
    expect(mnemonicOf('Back (focused pane)')).toBeNull();
  });
});
