// @vitest-environment jsdom
/** Opening an agent menu in the prompt bar must never drop the keyboard.
 *
 *  On iOS, a blurred prompt drops the keyboard. The shell then grows under a
 *  panel placed for the shrunk viewport, and the panel jumps. With the keyboard
 *  up, the menu's filter box takes it over, and closing hands it back. */
import { beforeAll, afterEach, beforeEach, describe, expect, it, vi } from 'vitest';
import { render } from 'preact';
import { act } from 'preact/test-utils';

vi.mock('../../../store/actions/models', async (importOriginal) => ({
  ...(await importOriginal<typeof import('../../../store/actions/models')>()),
  loadChatModels: vi.fn(async () => {}),
}));

const { CodingAgentControlMenu, menuTakesFocus } = await import('../CodingAgentControlMenu');
const { RungControl } = await import('../RungControl');
const { controlMenuParts } = await import('../controlMenuParts');

// The menus' panel parts are code-split. Warm them, so a press opens at once.
beforeAll(() => controlMenuParts.preload());

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
  ['the coding-agent menu', () => <CodingAgentControlMenu />, '.commands-btn'],
  ['the Lucidos Agent rung control', () => <RungControl codingAgent={null} composeContext />, '.rung-trigger'],
] as const;

const press = (el: Element) => {
  const down = new MouseEvent('mousedown', { bubbles: true, cancelable: true });
  el.dispatchEvent(down);
  return down;
};

describe.each(menus)('%s', (_name, menu, trigger) => {
  it('cancels the trigger mousedown, so it cannot move focus', () => {
    act(() => { render(menu(), host); });
    expect(press(host.querySelector(trigger)!).defaultPrevented).toBe(true);
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

describe('the rung list under a finger', () => {
  beforeEach(() => {
    vi.stubGlobal('ontouchstart', null);
    vi.stubGlobal('matchMedia', (query: string) => ({ matches: query === '(pointer: coarse)' }));
    vi.stubGlobal('fetch', vi.fn(async () => new Response(JSON.stringify({
      control_commands: [{
        subtype: 'set_model',
        label: 'Model',
        params: [{ key: 'model', options: [{ value: 'default', label: 'Default' }] }],
      }],
      builtin_commands: [],
      skill_commands: [],
      current_model: null,
      current_reasoning_effort: null,
      has_active_session: false,
      ladder_rungs: [],
    }), { status: 200 })));
  });

  const frame = () => act(() => new Promise((r) => requestAnimationFrame(() => r(undefined))));

  it.each([
    ['a coding agent', () => <RungControl codingAgent="claude-code" />],
    ['the Lucidos Agent', () => <RungControl codingAgent={null} composeContext />],
  ] as const)('leaves the keyboard with the prompt on %s', async (_name, control) => {
    const prompt = document.createElement('textarea');
    document.body.appendChild(prompt);
    await act(async () => { render(control(), host); });
    await frame();
    prompt.focus();

    act(() => { host.querySelector<HTMLButtonElement>('.rung-trigger')!.click(); });
    await frame();
    expect(document.querySelector('.control-option')).not.toBeNull();
    // The rung list has no field to type in, so a finger's keyboard has no
    // reason to leave the prompt.
    expect(document.querySelector('.control-filter')).toBeNull();
    expect(document.activeElement).toBe(prompt);
    prompt.remove();
  });
});
