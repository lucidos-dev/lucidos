// @vitest-environment jsdom
import { describe, it, expect, beforeEach } from 'vitest';
import { render } from 'preact';
import { act } from 'preact/test-utils';
import { KeyboardShortcutsSection } from '../KeyboardShortcutsSection';
import { findSettingsEntry } from '../../search/searchIndex';
import { SHORTCUT_DEFS } from '../../../utils/shortcuts';
import { preferences } from '../../../store/store';

let host: HTMLDivElement;

beforeEach(() => {
  host?.remove();
  host = document.createElement('div');
  document.body.appendChild(host);
  preferences.value = { status: 'loaded', data: {} };
  act(() => {
    render(<KeyboardShortcutsSection />, host);
  });
});

describe('a shortcut search result lands on its row', () => {
  it.each(SHORTCUT_DEFS.map((d) => d.id))('%s', (id) => {
    const anchor = findSettingsEntry(`shortcut:${id}`)?.anchor;
    expect(anchor, 'the search entry carries no anchor').toBeTruthy();
    const row = host.querySelector<HTMLElement>(`[data-search-anchor="${anchor}"]`);
    expect(row, 'no row renders the anchor').not.toBeNull();
    expect(row!.querySelector('button')?.getAttribute('aria-label')).toContain('Record a new shortcut');
  });
});
