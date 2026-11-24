// @vitest-environment jsdom
import { describe, it, expect, beforeEach, afterEach, vi } from 'vitest';
import { render } from 'preact';

vi.mock('../../../store/actions/repositories', () => ({
  viewChangeDiff: vi.fn(),
  viewThreadCcDiff: vi.fn(),
}));

import { DiffButton } from '../WaitingBanner';

let host: HTMLDivElement;

function button(): HTMLButtonElement {
  const btn = host.querySelector<HTMLButtonElement>('button[data-role="thread-diff"]');
  if (!btn) throw new Error('the prompt row draws no Diff');
  return btn;
}

beforeEach(() => {
  host = document.createElement('div');
  document.body.appendChild(host);
  render(<DiffButton threadId="tid" />, host);
});

afterEach(() => {
  render(null, host);
  host.remove();
});

describe('the prompt row draws Diff as an icon', () => {
  it('wears the row\'s icon-button classes and no action-btn pill', () => {
    expect(button().className).toContain('icon-btn');
    expect(button().className).toContain('header-icon');
    expect(button().className).not.toContain('action-btn');
  });

  it('shows a glyph and no text', () => {
    expect(button().querySelector('svg')).not.toBeNull();
    expect(button().textContent).toBe('');
  });

  it('draws three rows, not two', () => {
    expect(
      button().querySelectorAll('svg line[x1="10"]'),
      'counted at the body column x=10; move the count with it if the glyph moves',
    ).toHaveLength(3);
  });

  it('takes the row marker from the composer rather than hardcoding it', () => {
    expect(button().hasAttribute('data-row-item')).toBe(false);
  });
});

describe('the icon keeps the word it stopped showing', () => {
  it('names the action for a reader who cannot see the glyph', () => {
    expect(button().getAttribute('aria-label')).toBe('Diff');
  });

  it('carries a tooltip saying what it opens', () => {
    expect(button().getAttribute('data-tooltip')).toBeTruthy();
  });

  it('opts the tooltip into a touch long press', () => {
    expect(button().hasAttribute('data-tooltip-longpress')).toBe(true);
  });
});
