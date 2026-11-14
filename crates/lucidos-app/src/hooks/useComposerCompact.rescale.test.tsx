// @vitest-environment jsdom
/** A UI scale change moves the rem threshold without moving the row's px width,
 *  so no ResizeObserver fires. The hook re-measures on the font metrics instead.
 *  jsdom lays nothing out, so stubs give the row a width and the root a rem. */
import { afterEach, beforeEach, describe, expect, it, vi } from 'vitest';
import { render } from 'preact';
import { act } from 'preact/test-utils';
import { useRef } from 'preact/hooks';
import { COMPOSER_COMPACT_MAX_REM, useComposerCompact } from './useComposerCompact';

let remPx = 16;
vi.mock('../utils/dom', async (importOriginal) => ({
  ...(await importOriginal<typeof import('../utils/dom')>()),
  getRemPx: () => remPx,
}));

// Desktop faces at a 16px rem, compact faces at a 20px rem.
const ROW_WIDTH = COMPOSER_COMPACT_MAX_REM * 18;

let host: HTMLDivElement;
let compact: boolean | null = null;

function Row() {
  const ref = useRef<HTMLDivElement>(null);
  compact = useComposerCompact(ref);
  return <div ref={ref} />;
}

beforeEach(() => {
  remPx = 16;
  host = document.createElement('div');
  document.body.appendChild(host);
  vi.stubGlobal('ResizeObserver', class { observe() {} disconnect() {} });
  Object.defineProperty(document, 'fonts', {
    configurable: true,
    value: { addEventListener() {}, removeEventListener() {} },
  });
  Object.defineProperty(HTMLElement.prototype, 'clientWidth', { configurable: true, get: () => ROW_WIDTH });
});

afterEach(() => {
  render(null, host);
  host.remove();
  vi.unstubAllGlobals();
  Reflect.deleteProperty(HTMLElement.prototype, 'clientWidth');
  Reflect.deleteProperty(document, 'fonts');
  document.documentElement.style.removeProperty('--user-ui-scale');
});

describe('the composer row face set follows the UI scale', () => {
  it('turns compact when a larger scale lifts the threshold over the row', async () => {
    act(() => render(<Row />, host));
    expect(compact).toBe(false);

    remPx = 20;
    await act(async () => {
      document.documentElement.style.setProperty('--user-ui-scale', '1.25');
      await new Promise((resolve) => requestAnimationFrame(resolve));
      await new Promise((resolve) => requestAnimationFrame(resolve));
    });
    expect(compact).toBe(true);
  });
});
