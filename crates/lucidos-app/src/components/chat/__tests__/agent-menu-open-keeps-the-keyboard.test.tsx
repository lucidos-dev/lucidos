// @vitest-environment jsdom
/** Opening an agent menu in the prompt bar must never drop the keyboard.
 *
 *  On iOS, a blurred prompt drops the keyboard. The shell then grows under a
 *  panel placed for the shrunk viewport, and the panel jumps. With the keyboard
 *  up, the menu's filter box takes it over, and closing hands it back. */
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
  const takes = (over: Partial<Parameters<typeof menuTakesFocus>[0]>) =>
    menuTakesFocus({ coarsePointer: true, typed: false, keyboard: false, ...over });

  it('moves focus into the menu under a mouse', () => {
    expect(takes({ coarsePointer: false })).toBe(true);
  });

  it('leaves focus alone when a finger opened the menu with no keyboard up', () => {
    expect(takes({})).toBe(false);
  });

  it('takes the keyboard for the filter when a finger opened it mid-typing', () => {
    expect(takes({ keyboard: true })).toBe(true);
  });

  it('takes focus after a typed "/", so the typing lands in the filter', () => {
    expect(takes({ typed: true })).toBe(true);
  });
});

describe('the Lucidos Agent menu under a finger', () => {
  beforeEach(() => {
    vi.stubGlobal('ontouchstart', null);
    vi.stubGlobal('matchMedia', (query: string) => ({ matches: query === '(pointer: coarse)' }));
  });

  const openMenu = async () => {
    act(() => { host.querySelector<HTMLButtonElement>('.commands-btn')!.click(); });
    await act(() => new Promise((r) => requestAnimationFrame(() => r(undefined))));
  };

  it('hands the keyboard to the filter box, and back to the prompt on close', async () => {
    const prompt = document.createElement('textarea');
    document.body.appendChild(prompt);
    prompt.focus();
    act(() => { render(menus[1][1](), host); });
    await openMenu();

    const filter = document.querySelector('.control-filter');
    expect(filter).not.toBeNull();
    expect(document.activeElement).toBe(filter);

    const row = document.querySelector('.control-option');
    expect(row).not.toBeNull();
    expect(press(row!).defaultPrevented).toBe(true);
    expect(document.activeElement).toBe(filter);

    act(() => { host.querySelector<HTMLButtonElement>('.commands-btn')!.click(); });
    expect(document.activeElement).toBe(prompt);
    prompt.remove();
  });

  it('raises no keyboard when none was up', async () => {
    act(() => { render(menus[1][1](), host); });
    await openMenu();

    expect(document.querySelector('.control-filter')).not.toBeNull();
    expect(document.activeElement).toBe(document.body);
  });
});

describe('the coding-agent menu on its way back to the command list', () => {
  beforeEach(() => {
    vi.stubGlobal('ontouchstart', null);
    vi.stubGlobal('matchMedia', (query: string) => ({ matches: query === '(pointer: coarse)' }));
    vi.stubGlobal('fetch', vi.fn(async () => new Response(JSON.stringify({
      control_commands: [{
        subtype: 'set_model',
        label: 'Model',
        params: [{ key: 'model', options: [{ value: 'default', label: 'Default' }] }],
      }],
      builtin_commands: ['compact'],
      skill_commands: [],
      current_model: null,
      current_reasoning_effort: null,
      has_active_session: false,
    }), { status: 200 })));
  });

  const frame = () => act(() => new Promise((r) => requestAnimationFrame(() => r(undefined))));

  it('gives the keyboard back to the prompt before the step unmounts', async () => {
    const prompt = document.createElement('textarea');
    document.body.appendChild(prompt);
    await act(async () => { render(<CodingAgentControlMenu />, host); });
    await frame();
    prompt.focus();

    act(() => { host.querySelector<HTMLButtonElement>('.commands-btn')!.click(); });
    await frame();
    const row = Array.from(document.querySelectorAll<HTMLButtonElement>('.control-item'))
      .find((b) => b.textContent?.trim() === 'Model');
    expect(row).toBeDefined();
    act(() => { row!.click(); });
    await frame();
    // A tap into the model picker's own filter box moves focus into the panel.
    const pickerFilter = document.querySelector<HTMLInputElement>('input[placeholder="Filter models…"]');
    expect(pickerFilter).not.toBeNull();
    pickerFilter!.focus();

    // The list's own filter may take focus once it is back, so the end state
    // cannot tell. What must happen is the prompt taking it in between.
    let promptTookFocus = false;
    prompt.addEventListener('focus', () => { promptTookFocus = true; });
    act(() => { document.querySelector<HTMLButtonElement>('.control-back')!.click(); });
    expect(promptTookFocus, 'the focused box unmounted, taking the keyboard').toBe(true);
    expect(document.activeElement?.tagName).toMatch(/^(TEXTAREA|INPUT)$/);
    prompt.remove();
  });
});
