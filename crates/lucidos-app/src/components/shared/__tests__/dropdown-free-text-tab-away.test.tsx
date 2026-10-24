// @vitest-environment jsdom
/** Tab away from an open free-text dropdown must leave focus where Tab put it.
 *
 *  The open menu defers focusing its input by one frame. A frame still pending
 *  when Tab closed the menu pulled focus back into the input, whose focus
 *  handler reopened the menu: Tab looked dead and the menu stayed up. */
import { afterEach, beforeEach, expect, it, vi } from 'vitest';
import { render } from 'preact';
import { act } from 'preact/test-utils';
import { Dropdown } from '../Dropdown';
import { handleOverlayTab } from '../overlayFocus';

let host: HTMLDivElement;
let frames: Array<FrameRequestCallback | null>;

beforeEach(() => {
  host = document.createElement('div');
  document.body.appendChild(host);
  frames = [];
  vi.stubGlobal('requestAnimationFrame', (cb: FrameRequestCallback) => frames.push(cb));
  vi.stubGlobal('cancelAnimationFrame', (id: number) => { frames[id - 1] = null; });
});

afterEach(() => {
  render(null, host);
  host.remove();
  vi.unstubAllGlobals();
});

function runFrames() {
  for (const frame of frames.splice(0)) frame?.(0);
}

it('a frame pending when Tab closes the menu does not pull focus back', () => {
  act(() => {
    render(
      <div>
        <button id="before">before</button>
        <Dropdown freeText options={[{ value: '500', label: '500' }]} value="500" onChange={() => {}} />
      </div>,
      host,
    );
  });
  const input = host.querySelector<HTMLInputElement>('.dropdown-input')!;
  const before = host.querySelector<HTMLButtonElement>('#before')!;
  act(() => { input.focus(); });
  expect(document.querySelector('.dropdown-menu')).not.toBeNull();

  act(() => {
    before.focus();
    handleOverlayTab(new KeyboardEvent('keydown', { key: 'Tab', shiftKey: true, cancelable: true }));
  });
  act(() => { runFrames(); });

  expect(document.activeElement).toBe(before);
  expect(document.querySelector('.dropdown-menu')).toBeNull();
});
