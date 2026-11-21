// @vitest-environment jsdom
/** A face flip changes every member's width, so the fold re-measures and never
 *  decides against a width the old face set remembered. jsdom lays nothing out,
 *  so a stub gives each member a width that depends on the row's face set. */
import { afterEach, beforeEach, describe, expect, it, vi } from 'vitest';
import { render } from 'preact';
import { act } from 'preact/test-utils';
import { useRef } from 'preact/hooks';
import { FOLD_KEY_ATTR, usePromptActionCollapse } from './usePromptActionCollapse';

const KEYS = ['a', 'b'];
const GROUPS = ['row', 'row'] as const;
const DESKTOP_WIDTH = 40;
const COMPACT_WIDTH = 10;
const PINNED_WIDTH = 10;
const ROW_WIDTH = 70;

let host: HTMLDivElement;
const realRect = HTMLElement.prototype.getBoundingClientRect;

beforeEach(() => {
  host = document.createElement('div');
  document.body.appendChild(host);
  vi.stubGlobal('ResizeObserver', class { observe() {} disconnect() {} });
  Object.defineProperty(HTMLElement.prototype, 'clientWidth', { configurable: true, get: () => ROW_WIDTH });
  HTMLElement.prototype.getBoundingClientRect = function rect(this: HTMLElement) {
    const compact = this.closest('[data-composer-compact]') !== null;
    const width = this.hasAttribute(FOLD_KEY_ATTR) ? (compact ? COMPACT_WIDTH : DESKTOP_WIDTH) : PINNED_WIDTH;
    return { width } as DOMRect;
  };
});

afterEach(() => {
  render(null, host);
  host.remove();
  vi.unstubAllGlobals();
  HTMLElement.prototype.getBoundingClientRect = realRect;
  Reflect.deleteProperty(HTMLElement.prototype, 'clientWidth');
});

function Row({ compact }: { compact: boolean }) {
  const ref = useRef<HTMLDivElement>(null);
  const { folded } = usePromptActionCollapse(ref, KEYS, GROUPS, null, compact);
  return (
    <div ref={ref} data-composer-compact={compact || undefined} data-folded={folded}>
      {KEYS.slice(folded).map((key) => <span key={key} data-row-item="fold" {...{ [FOLD_KEY_ATTR]: key }} />)}
      <button data-row-item />
    </div>
  );
}

const foldedCount = () => Number(host.querySelector('[data-folded]')!.getAttribute('data-folded'));

async function draw(compact: boolean) {
  await act(async () => {
    render(<Row compact={compact} />, host);
    await Promise.resolve();
  });
}

describe('the fold follows the face set', () => {
  it('unfolds when the compact faces fit, and folds again when the desktop ones return', async () => {
    await draw(false);
    expect(foldedCount()).toBe(2);
    await draw(true);
    expect(foldedCount()).toBe(0);
    await draw(false);
    expect(foldedCount()).toBe(2);
  });
});
