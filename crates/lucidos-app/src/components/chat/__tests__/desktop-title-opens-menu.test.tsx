// @vitest-environment jsdom
/**
 * On desktop the thread title is a menu button too. A left-click toggles the
 * thread menu and a right-click opens it, as on the mobile row. The pin stays
 * beside it, and the row draws no ⋯.
 */
import { describe, it, expect, beforeEach, afterEach, vi } from 'vitest';
import { render } from 'preact';
import { act } from 'preact/test-utils';
import { DesktopThreadTitleBar } from '../ThreadView';
import { threadMap } from '../../../store/store';
import { makeThreadState } from '../../../store/actions/threads-test-helpers';

vi.mock('../../../store/actions/threads', () => ({
  focusThread: vi.fn(),
  handleSaveThread: vi.fn(),
  handleUnsaveThread: vi.fn(),
}));

const THREAD = 'thread-1';
const TITLE = 'Fix number alignment';
let host: HTMLDivElement;

function titleButton(): HTMLButtonElement {
  const el = host.querySelector<HTMLButtonElement>('.thread-view-header .thread-title-menu');
  expect(el, 'the desktop title is not a menu button').not.toBeNull();
  return el as HTMLButtonElement;
}

function openMenus(): number {
  return document.querySelectorAll('.thread-overflow-menu').length;
}

async function dispatch(target: HTMLElement, type: string, init: MouseEventInit = {}): Promise<MouseEvent> {
  const e = new MouseEvent(type, { bubbles: true, cancelable: true, ...init });
  await act(() => { target.dispatchEvent(e); });
  return e;
}

beforeEach(async () => {
  threadMap.value = new Map([[THREAD, makeThreadState(THREAD, { meta: { title: TITLE } })]]);
  host = document.createElement('div');
  document.body.appendChild(host);
  await act(() => { render(<DesktopThreadTitleBar threadId={THREAD} />, host); });
});

afterEach(() => {
  render(null, host);
  host.remove();
  document.querySelectorAll('.thread-overflow-menu').forEach((el) => el.remove());
  threadMap.value = new Map();
});

describe('the desktop title', () => {
  it('is a menu button named by the title', () => {
    const button = titleButton();
    expect(button.textContent).toContain(TITLE);
    expect(button.getAttribute('aria-haspopup')).toBe('menu');
    expect(button.getAttribute('aria-expanded')).toBe('false');
  });

  it('keeps the pin beside it, and draws no ⋯', () => {
    expect(host.querySelector('.thread-view-header .pin-thread-btn')).not.toBeNull();
    const menuButtons = host.querySelectorAll('.thread-view-header button[aria-haspopup="menu"]');
    expect(menuButtons).toHaveLength(1);
    expect(menuButtons[0]).toBe(titleButton());
  });

  it('opens the thread menu on a left-click, and closes it on a second', async () => {
    await dispatch(titleButton(), 'click', { detail: 1 });
    expect(openMenus()).toBe(1);
    expect(titleButton().getAttribute('aria-expanded')).toBe('true');
    await dispatch(titleButton(), 'click', { detail: 1 });
    expect(openMenus()).toBe(0);
  });

  it('opens the thread menu on a right-click, instead of the browser menu', async () => {
    const e = await dispatch(titleButton(), 'contextmenu', { clientX: 40, clientY: 10 });
    expect(openMenus()).toBe(1);
    expect(e.defaultPrevented).toBe(true);
  });
});
