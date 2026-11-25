// @vitest-environment jsdom
import { describe, it, expect, beforeEach, afterEach, vi } from 'vitest';
import { render } from 'preact';
import { PREF_THREAD_FOLDERS } from '@lucidos/preference-catalog';
import { PinThreadButton } from '../PinThreadButton';
import { preferences, threadMap } from '../../../store/store';
import { makeThreadState } from '../../../store/actions/threads-test-helpers';
import { handleSaveThread } from '../../../store/actions/threads';

vi.mock('../../../store/actions/threads', () => ({
  handleSaveThread: vi.fn(),
  handleUnsaveThread: vi.fn(),
}));

let host: HTMLDivElement;

function setFolders(raw: string | null): void {
  preferences.value = { status: 'loaded', data: raw === null ? {} : { [PREF_THREAD_FOLDERS.key]: raw } };
}

beforeEach(() => {
  vi.mocked(handleSaveThread).mockReset();
  threadMap.value = new Map([
    ['t', makeThreadState('t')],
    ['child', makeThreadState('child', { meta: { parentThreadId: 't' } })],
  ]);
  host = document.createElement('div');
  document.body.appendChild(host);
});

afterEach(() => {
  render(null, host);
  host.remove();
});

describe('the pin button', () => {
  it('pins in one tap while no custom folder exists', () => {
    setFolders(null);
    render(<PinThreadButton threadId="t" saved={false} />, host);
    const button = host.querySelector<HTMLButtonElement>('.pin-btn')!;
    expect(button.getAttribute('aria-haspopup')).toBeNull();
    button.click();
    expect(handleSaveThread).toHaveBeenCalledWith('t');
  });

  it('opens a menu instead once a custom folder exists', () => {
    setFolders('[{"id":"w48","name":"W48"}]');
    render(<PinThreadButton threadId="t" saved={false} />, host);
    const button = host.querySelector<HTMLButtonElement>('.pin-btn')!;
    expect(button.getAttribute('aria-haspopup')).toBe('menu');
  });

  it('pins a child in one tap, since its family follows the root', () => {
    setFolders('[{"id":"w48","name":"W48"}]');
    render(<PinThreadButton threadId="child" saved={false} />, host);
    expect(host.querySelector('.pin-btn')!.getAttribute('aria-haspopup')).toBeNull();
  });

  it('still unpins in one tap with folders', () => {
    setFolders('[{"id":"w48","name":"W48"}]');
    render(<PinThreadButton threadId="t" saved />, host);
    expect(host.querySelector('.pin-btn')!.getAttribute('aria-haspopup')).toBeNull();
  });
});
