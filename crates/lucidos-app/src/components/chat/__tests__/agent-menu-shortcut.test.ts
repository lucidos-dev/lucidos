// @vitest-environment jsdom
/** The model-and-effort shortcut presses the composer row's own button.
 *
 *  So the list opens exactly as a click opens it. Only the rung control holds
 *  the model: the row's `/` anchor never opens it. A button laid out at zero
 *  size is not the one on screen.
 *
 *  Plan: `docs/plans/2026-10-01-shortcuts-for-every-toggle.md`. */
import { describe, it, expect, beforeEach, vi } from 'vitest';

vi.mock('../../../utils/elementVisibility', async (importOriginal) => {
  const actual = await importOriginal<typeof import('../../../utils/elementVisibility')>();
  // jsdom lays nothing out, so visibility rides a test attribute.
  return { ...actual, isElementVisible: (el: HTMLElement) => el.dataset.visible === 'yes' };
});

import { openAgentMenu } from '../promptFocus';
import { rungFoldOpenRequest } from '../controlMenuRequests';

function anchor(visible: boolean, className = 'rung-trigger'): { button: HTMLButtonElement; clicks: () => number } {
  const row = document.createElement('div');
  row.className = 'prompt-actions-row';
  const button = document.createElement('button');
  button.className = className;
  button.dataset.visible = visible ? 'yes' : 'no';
  let count = 0;
  button.addEventListener('click', () => { count++; });
  row.appendChild(button);
  document.body.appendChild(row);
  return { button, clicks: () => count };
}

beforeEach(() => {
  document.body.innerHTML = '';
  rungFoldOpenRequest.value = null;
});

describe('openAgentMenu', () => {
  it('presses the visible rung control and skips one laid out at zero size', () => {
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

  it('never presses the `/` anchor, which holds no model', () => {
    const slash = anchor(true, 'icon-btn header-icon commands-btn');
    openAgentMenu();
    expect(slash.clicks()).toBe(0);
  });

  it('asks for the list against the ⋯ when the row has folded the pill', () => {
    const more = anchor(true, 'prompt-actions-more');
    more.button.setAttribute('data-fold-key', '');
    openAgentMenu();
    expect(rungFoldOpenRequest.value).toBe(more.button);
    expect(more.clicks()).toBe(0);
  });
});
