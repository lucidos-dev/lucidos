// @vitest-environment jsdom
/**
 * The users icon and panel draw only on a team gateway (ADR 0649). On a
 * personal gateway they read nothing and draw nothing, so it looks exactly as
 * before. On a team they read the engine and draw what it says.
 */
import { afterEach, beforeEach, describe, expect, it, vi } from 'vitest';
import { render } from 'preact';
import { act } from 'preact/test-utils';

vi.mock('../../../api/client', async (original) => ({
  ...(await original<typeof import('../../../api/client')>()),
  readThreadUsers: vi.fn(async () => ({
    root_thread_id: 'thread-1',
    driver: { id: 'sam', name: 'Sam' },
    watchers: [{ id: 'kim', name: 'Kim' }],
  })),
  changeThreadRole: vi.fn(async () => ({
    root_thread_id: 'thread-1',
    driver: { id: 'kim', name: 'Kim' },
    watchers: [{ id: 'kim', name: 'Kim' }],
  })),
}));
const { openSettingsSubview } = vi.hoisted(() => ({ openSettingsSubview: vi.fn() }));
vi.mock('../../../store/actions/menu', () => ({ openSettingsSubview }));

import { changeThreadRole, readThreadUsers } from '../../../api/client';
import { ThreadUsersButton, ThreadUsersPanel } from '../ThreadUsers';
import { users, usersPanelOpenFor } from '../../../store/actions/users';
import { focusedThreadId, threadMap } from '../../../store/store';
import { makeThreadState } from '../../../store/actions/threads-test-helpers';

const THREAD = 'thread-1';
let host: HTMLDivElement;

async function draw(): Promise<void> {
  await act(async () => {
    render(<><ThreadUsersButton threadId={THREAD} /><ThreadUsersPanel threadId={THREAD} /></>, host);
  });
  await act(async () => { await Promise.resolve(); });
}

function teamAs(you: string): void {
  users.value = {
    status: 'loaded',
    data: { users: [{ id: 'sam', name: 'Sam', has_device: true }, { id: 'kim', name: 'Kim', has_device: true }], you, team: true },
  };
}

const button = () => host.querySelector<HTMLButtonElement>('[data-role="thread-users-button"]');
const panel = () => host.querySelector('[data-role="thread-users-panel"]');
const panelButtons = () => [...panel()!.querySelectorAll('button')].map((b) => b.textContent);

beforeEach(() => {
  threadMap.value = new Map([[THREAD, makeThreadState(THREAD, { meta: { state: 'active' } })]]);
  focusedThreadId.value = THREAD;
  host = document.createElement('div');
  document.body.appendChild(host);
  vi.mocked(readThreadUsers).mockClear();
  vi.mocked(changeThreadRole).mockClear();
  openSettingsSubview.mockClear();
});

afterEach(() => {
  render(null, host);
  host.remove();
  users.value = { status: 'not-loaded' };
  usersPanelOpenFor.value = null;
  threadMap.value = new Map();
  focusedThreadId.value = null;
});

describe('on a personal gateway', () => {
  it('reads nothing and draws neither the icon nor the panel', async () => {
    users.value = { status: 'loaded', data: { users: [{ id: 'me', name: 'Me', has_device: true }], you: 'me', team: false } };
    await draw();
    expect(readThreadUsers).not.toHaveBeenCalled();
    expect(button()).toBeNull();
    expect(panel()).toBeNull();
  });
});

describe('on a team gateway', () => {
  it('draws the driver as one icon, with the panel closed', async () => {
    teamAs('kim');
    await draw();
    expect(button()?.textContent).toContain('S');
    expect(button()?.getAttribute('aria-expanded')).toBe('false');
    expect(button()?.getAttribute('aria-label')).toBe('Users in this thread');
    expect(panel()).toBeNull();
  });

  it('rolls the panel open with a row per user and this user\'s own actions', async () => {
    teamAs('kim');
    await draw();
    await act(async () => { button()!.click(); });
    expect(button()?.getAttribute('aria-expanded')).toBe('true');
    const rows = [...panel()!.querySelectorAll('.thread-users-user')].map((r) => r.textContent);
    expect(rows).toEqual(['SSamdriving', 'KYouwatching']);
    expect(panelButtons()).toEqual(['Unwatch', 'Take', 'Edit users']);
  });

  it('keeps Take reaching the engine and lands its answer', async () => {
    teamAs('kim');
    await draw();
    await act(async () => { button()!.click(); });
    await act(async () => { [...panel()!.querySelectorAll('button')].find((b) => b.textContent === 'Take')!.click(); });
    expect(changeThreadRole).toHaveBeenCalledWith(THREAD, 'take');
    expect(panelButtons()).toEqual(['Unwatch', 'Release', 'Edit users']);
  });

  it('opens the Settings subview holding the users from Edit users', async () => {
    teamAs('kim');
    await draw();
    await act(async () => { button()!.click(); });
    await act(async () => { [...panel()!.querySelectorAll('button')].find((b) => b.textContent === 'Edit users')!.click(); });
    expect(openSettingsSubview).toHaveBeenCalledWith('users');
  });
});
