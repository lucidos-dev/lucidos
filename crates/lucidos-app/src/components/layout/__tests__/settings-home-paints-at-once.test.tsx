// @vitest-environment jsdom
/** Opening Settings draws its home list in the same render, with no chunk to
 *  wait for. A lazy component renders nothing until its chunk lands. */
import { afterEach, beforeEach, expect, it, vi } from 'vitest';
import { render } from 'preact';
import { act } from 'preact/test-utils';

vi.mock('../../../store/actions/settingsReads', () => ({ warmSettingsReads: () => {} }));

import { ContentPane } from '../ContentPane';
import { activeMenuItem, panelOverlay, settingsSubview, SETTINGS_NAV_ITEMS } from '../../../store/store';

let host: HTMLDivElement;

beforeEach(() => {
  host = document.createElement('div');
  document.body.appendChild(host);
  panelOverlay.value = null;
  activeMenuItem.value = 'settings';
  settingsSubview.value = 'main';
});

afterEach(() => {
  render(null, host);
  host.remove();
});

it('draws every Settings category on the first render', () => {
  act(() => { render(<ContentPane layout="desktop" />, host); });
  const rows = host.querySelectorAll('.settings-nav-row');
  expect(rows).toHaveLength(SETTINGS_NAV_ITEMS.length);
  expect(rows[0]?.textContent).toContain(SETTINGS_NAV_ITEMS[0]!.label);
});
