// @vitest-environment jsdom
/** The rung list opens with the rung in force mid-panel, so its neighbours
 *  show on both sides. After that the arrow keys only nudge the row into view.
 *  The centring scrolls the panel alone, never the page. */
import { afterEach, beforeEach, describe, expect, it, vi } from 'vitest';
import { render } from 'preact';
import { act } from 'preact/test-utils';
import { RungPicker } from '../RungPicker';

let host: HTMLDivElement;
let scrolls: { row: string | null; block: ScrollLogicalPosition | undefined }[];

beforeEach(() => {
  scrolls = [];
  vi.spyOn(Element.prototype, 'scrollIntoView').mockImplementation(function (this: Element, arg?: boolean | ScrollIntoViewOptions) {
    const block = typeof arg === 'object' ? arg.block : undefined;
    scrolls.push({ row: this.textContent, block });
  });
  host = document.createElement('div');
  host.className = 'control-dropdown';
  document.body.appendChild(host);
});

afterEach(() => {
  render(null, host);
  host.remove();
  vi.restoreAllMocks();
});

const rows = ['low', 'medium', 'high', 'xhigh', 'max'].map((effort) => ({
  value: effort,
  label: `Opus 5.5 · ${effort}`,
}));

// Each row is 20px tall, stacked from the panel's top; the panel shows 60px.
const ROW_HEIGHT = 20;
const PANEL_HEIGHT = 60;

function fakeGeometry() {
  Object.defineProperty(host, 'clientHeight', { configurable: true, value: PANEL_HEIGHT });
  vi.spyOn(HTMLElement.prototype, 'offsetHeight', 'get').mockReturnValue(ROW_HEIGHT);
  vi.spyOn(Element.prototype, 'getBoundingClientRect').mockImplementation(function (this: Element) {
    const index = rows.findIndex((r) => this.textContent?.includes(r.label));
    const top = this === host || index < 0 ? 0 : index * ROW_HEIGHT;
    return { top, bottom: top + ROW_HEIGHT, left: 0, right: 0, width: 0, height: ROW_HEIGHT, x: 0, y: top, toJSON: () => ({}) };
  });
}

describe('the rung list', () => {
  it('opens with the rung in force centred, then nudges it on an arrow key', () => {
    fakeGeometry();
    act(() => {
      render(<RungPicker label="Model" rows={rows} current="high" onPick={() => {}} />, host);
    });
    // "high" is row 2 (top 40px): centred in 60px, its top sits at 20px.
    expect(host.scrollTop).toBe(2 * ROW_HEIGHT - (PANEL_HEIGHT - ROW_HEIGHT) / 2);
    expect(scrolls).toEqual([]);

    const list = host.querySelector<HTMLElement>('.control-list')!;
    act(() => {
      list.dispatchEvent(new KeyboardEvent('keydown', { key: 'ArrowDown', bubbles: true, cancelable: true }));
    });
    expect(scrolls[scrolls.length - 1]).toEqual({ row: expect.stringContaining('Opus 5.5 · xhigh'), block: 'nearest' });
  });
});
