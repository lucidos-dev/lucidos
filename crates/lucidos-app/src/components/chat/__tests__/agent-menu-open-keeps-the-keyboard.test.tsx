// @vitest-environment jsdom
/** Opening an agent menu in the prompt bar must not take focus from the prompt.
 *
 *  On iOS, losing focus drops the keyboard. The shell then grows under a panel
 *  placed for the shrunk viewport, and the panel jumps. */
import { afterEach, beforeEach, describe, expect, it, vi } from 'vitest';
import { render } from 'preact';
import { act } from 'preact/test-utils';

vi.mock('../../../store/actions/models', async (importOriginal) => ({
  ...(await importOriginal<typeof import('../../../store/actions/models')>()),
  loadChatModels: vi.fn(async () => {}),
}));

const { CodingAgentControlMenu, menuTakesFocus } = await import('../CodingAgentControlMenu');
const { LucidosControlMenu } = await import('../LucidosControlMenu');

let host: HTMLDivElement;

beforeEach(() => {
  vi.stubGlobal('fetch', vi.fn(() => new Promise(() => {})));
  host = document.createElement('div');
  document.body.appendChild(host);
});

afterEach(() => {
  render(null, host);
  host.remove();
  vi.unstubAllGlobals();
});

const menus = [
  ['the coding-agent menu', () => <CodingAgentControlMenu />],
  ['the Lucidos Agent menu', () => <LucidosControlMenu composeContext />],
] as const;

const press = (el: Element) => {
  const down = new MouseEvent('mousedown', { bubbles: true, cancelable: true });
  el.dispatchEvent(down);
  return down;
};

describe.each(menus)('%s', (_name, menu) => {
  it('cancels the trigger mousedown, so it cannot move focus', () => {
    act(() => { render(menu(), host); });
    expect(press(host.querySelector('.commands-btn')!).defaultPrevented).toBe(true);
  });
});

describe('menuTakesFocus', () => {
  it('moves focus into the menu under a mouse', () => {
    expect(menuTakesFocus({ coarsePointer: false, typed: false })).toBe(true);
  });

  it('leaves focus in the prompt when a finger opened the menu', () => {
    expect(menuTakesFocus({ coarsePointer: true, typed: false })).toBe(false);
  });

  it('takes focus after a typed "/", so the typing lands in the filter', () => {
    expect(menuTakesFocus({ coarsePointer: true, typed: true })).toBe(true);
  });
});

describe('the Lucidos Agent menu under a finger', () => {
  beforeEach(() => {
    vi.stubGlobal('ontouchstart', null);
    vi.stubGlobal('matchMedia', (query: string) => ({ matches: query === '(pointer: coarse)' }));
  });

  it('leaves the prompt focused through open and a row press', async () => {
    const prompt = document.createElement('textarea');
    document.body.appendChild(prompt);
    prompt.focus();
    act(() => { render(menus[1][1](), host); });
    act(() => { host.querySelector<HTMLButtonElement>('.commands-btn')!.click(); });
    await act(() => new Promise((r) => requestAnimationFrame(() => r(undefined))));

    const row = document.querySelector('.control-option');
    expect(row).not.toBeNull();
    expect(press(row!).defaultPrevented).toBe(true);
    expect(document.activeElement).toBe(prompt);
    prompt.remove();
  });
});
