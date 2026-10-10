// @vitest-environment jsdom
/** The agent-menu shortcut presses the composer row's own anchor.
 *
 *  So the menu opens exactly as a click opens it, on whichever backend the
 *  row anchors. A button laid out at zero size is not the one on screen.
 *
 *  Plan: `docs/plans/2026-10-01-shortcuts-for-every-toggle.md`. */
import { describe, it, expect, beforeEach, vi } from 'vitest';

vi.mock('../scrollState', async (importOriginal) => {
  const actual = await importOriginal<typeof import('../scrollState')>();
  // jsdom lays nothing out, so visibility rides a test attribute.
  return { ...actual, isElementVisible: (el: HTMLElement) => el.dataset.visible === 'yes' };
});

import { openAgentMenu } from '../promptFocus';

function anchor(visible: boolean): { button: HTMLButtonElement; clicks: () => number } {
  const row = document.createElement('div');
  row.className = 'prompt-actions-row';
  const button = document.createElement('button');
  button.className = 'icon-btn header-icon commands-btn';
  button.dataset.visible = visible ? 'yes' : 'no';
  let count = 0;
  button.addEventListener('click', () => { count++; });
  row.appendChild(button);
  document.body.appendChild(row);
  return { button, clicks: () => count };
}

beforeEach(() => {
  document.body.innerHTML = '';
});

describe('openAgentMenu', () => {
  it('presses the visible anchor and skips one laid out at zero size', () => {
    const hidden = anchor(false);
    const shown = anchor(true);
    openAgentMenu();
    expect(hidden.clicks()).toBe(0);
    expect(shown.clicks()).toBe(1);
  });

  it('does nothing when no composer row is on screen', () => {
    const hidden = anchor(false);
    openAgentMenu();
    expect(hidden.clicks()).toBe(0);
  });
});
