import { describe, it, expect, beforeEach, afterEach, vi } from 'vitest';

const mocks = vi.hoisted(() => ({
  isTauri: vi.fn(() => true),
  invoke: vi.fn(() => Promise.resolve(false)),
  showToast: vi.fn(),
}));

vi.mock('../../utils/platform', () => ({ isTauri: mocks.isTauri }));
vi.mock('../../utils/tauri', () => ({ invoke: mocks.invoke }));
vi.mock('../store', async (importOriginal) => ({
  ...(await importOriginal<typeof import('../store')>()),
  showToast: mocks.showToast,
}));

import { engineStartedAt } from '../store';
import { installDevWindowFollow } from './dev-window';

const FIRST = '2026-10-12T08:00:00Z';
const SWITCHED = '2026-10-12T09:30:00Z';

describe('installDevWindowFollow', () => {
  let stop: () => void = () => {};

  beforeEach(() => {
    vi.clearAllMocks();
    mocks.isTauri.mockReturnValue(true);
    mocks.invoke.mockReturnValue(Promise.resolve(false));
    engineStartedAt.value = null;
  });

  afterEach(() => {
    stop();
    engineStartedAt.value = null;
  });

  it('tells the shell about the first engine start and every restart', () => {
    stop = installDevWindowFollow();
    expect(mocks.invoke).not.toHaveBeenCalled();

    engineStartedAt.value = FIRST;
    engineStartedAt.value = SWITCHED;

    expect(mocks.invoke.mock.calls).toEqual([
      ['follow_engine_switch', { engineStartedAtMs: Date.parse(FIRST) }],
      ['follow_engine_switch', { engineStartedAtMs: Date.parse(SWITCHED) }],
    ]);
  });

  it('sends a start that was already known when it installed', () => {
    engineStartedAt.value = FIRST;
    stop = installDevWindowFollow();
    expect(mocks.invoke).toHaveBeenCalledWith('follow_engine_switch', {
      engineStartedAtMs: Date.parse(FIRST),
    });
  });

  it('does nothing outside the desktop shell', () => {
    mocks.isTauri.mockReturnValue(false);
    stop = installDevWindowFollow();
    engineStartedAt.value = FIRST;
    expect(mocks.invoke).not.toHaveBeenCalled();
  });

  it('skips a start it cannot read as a time', () => {
    stop = installDevWindowFollow();
    engineStartedAt.value = 'not a time';
    expect(mocks.invoke).not.toHaveBeenCalled();
  });

  it('says so when the window could not move', async () => {
    mocks.invoke.mockReturnValue(Promise.reject('exec failed: permission denied'));
    stop = installDevWindowFollow();
    engineStartedAt.value = SWITCHED;
    await vi.waitFor(() => expect(mocks.showToast).toHaveBeenCalled());
    expect(mocks.showToast).toHaveBeenCalledWith(
      'The dev window could not move to the new version: exec failed: permission denied',
      'error',
    );
  });
});
