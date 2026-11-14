// @vitest-environment jsdom
/** The prompt bar's rung list filters as the user types, the way the model
 *  picker and every other dropdown do. The ladder link survives any query. */
import { afterEach, beforeEach, describe, expect, it, vi } from 'vitest';
import { render } from 'preact';
import { act } from 'preact/test-utils';
import { RungPicker } from '../RungPicker';
import { LADDER_LINK_LABEL, LADDER_LINK_ROW } from '../rungRows';

// A desktop with a mouse: the box waits for a keystroke there.
vi.mock('../../../utils/viewport', async (importOriginal) => ({
  ...(await importOriginal<typeof import('../../../utils/viewport')>()),
  isTouchDevice: () => false,
  hasCoarsePointer: () => false,
}));

let host: HTMLDivElement;

beforeEach(() => {
  vi.spyOn(Element.prototype, 'scrollIntoView').mockImplementation(() => {});
  host = document.createElement('div');
  host.className = 'control-dropdown';
  document.body.appendChild(host);
});

afterEach(() => {
  render(null, host);
  host.remove();
  vi.restoreAllMocks();
});

const rows = [
  { value: 'haiku|low', label: 'Haiku 5.5 · low' },
  { value: 'opus|medium', label: 'Opus 5.5 · medium' },
  { value: 'opus|high', label: 'Opus 5.5 · high' },
  LADDER_LINK_ROW,
];

const labels = () => [...host.querySelectorAll('.control-option-label')].map((el) => el.textContent?.replace('✓', ''));

function type(target: Element, key: string) {
  act(() => {
    target.dispatchEvent(new KeyboardEvent('keydown', { key, bubbles: true, cancelable: true }));
  });
}

describe('the rung list filter', () => {
  it('draws no box until a key is typed, then narrows to the matching rungs', () => {
    const onPick = vi.fn();
    act(() => {
      render(<RungPicker label="Model" rows={rows} current="opus|medium" onPick={onPick} />, host);
    });
    expect(host.querySelector('input')).toBeNull();

    const list = host.querySelector('.control-list')!;
    for (const key of 'opus') type(list, key);
    expect(host.querySelector<HTMLInputElement>('input')?.value).toBe('opus');
    expect(labels()).toEqual(['Opus 5.5 · medium', 'Opus 5.5 · high', LADDER_LINK_LABEL]);

    // Enter picks the first match, as the highlight reset to the top.
    type(list, 'Enter');
    expect(onPick).toHaveBeenCalledWith('opus|medium');
  });

  it('keeps the ladder link when no rung matches', () => {
    act(() => {
      render(<RungPicker label="Model" rows={rows} current="opus|medium" onPick={() => {}} />, host);
    });
    const list = host.querySelector('.control-list')!;
    for (const key of 'grok') type(list, key);
    expect(labels()).toEqual([LADDER_LINK_LABEL]);
  });
});
