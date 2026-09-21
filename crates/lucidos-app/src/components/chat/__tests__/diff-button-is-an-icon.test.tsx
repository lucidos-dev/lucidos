// @vitest-environment jsdom
/**
 * The composer's Diff is an ICON, beside the standing apply it now matches.
 *
 * It was a blue `action-btn` pill, and the widest control the prompt row
 * carried. The row's lift machinery exists to absorb that width, so on a phone
 * at a raised ui-scale the pill spent a sub-row of its own.
 *
 * This file pins what an icon-only button owes: the word, for a reader who
 * cannot see the glyph, and a tooltip a finger can reach.
 */
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

  // The glyph is a hunk: an added line, a context line, a removed line. Two
  // rows read as squat beside the icons either side of it, which is what the
  // report was about. It counts the body column every row shares, so the
  // markers and the exact heights stay free to move.
  it('draws three rows, not two', () => {
    expect(
      button().querySelectorAll('svg line[x1="10"]'),
      'counted at the body column x=10; move the count with it if the glyph moves',
    ).toHaveLength(3);
  });

  // useFitsInOneRow sums every [data-row-item]; a control missing the
  // attribute lets the row overflow instead of lifting its liftable slot.
  /** The composer stamps the measurement marker, because it also names WHICH
   *  member this is. A hardcoded one here would win over that name and hide the
   *  member from the fold. */
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

  // The phone is why the label went, so the tooltip has to reach a finger. The
  // host shell reveals on a long press only for elements that opt in.
  it('opts the tooltip into a touch long press', () => {
    expect(button().hasAttribute('data-tooltip-longpress')).toBe(true);
  });
});
