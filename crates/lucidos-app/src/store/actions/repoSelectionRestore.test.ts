import { beforeEach, expect, it, vi } from 'vitest';
const m = vi.hoisted(() => ({ selected: 'change-one' as string | null, menu: 'files', get: vi.fn(), preload: vi.fn(), apply: vi.fn(), toast: vi.fn() }));
vi.mock('../../utils/windowStorage', () => ({ readWindowItem: () => m.selected, removeWindowItem: () => { m.selected = null; } }));
vi.mock('../store', () => ({ panelOverlay: { value: null }, parseRepoPath: () => null, SELECTED_CHANGE_KEY: 'selected-change', activeMenuItem: { peek: () => m.menu }, showToast: m.toast }));
vi.mock('../../api/client', () => ({
  getChangeById: m.get,
  ApiError: class extends Error { constructor(public httpCode: number, message: string) { super(message); } },
}));
vi.mock('../../utils/lazyComponent', () => ({ lazyModule: () => ({ preload: m.preload, get: () => ({ loadChangeContext: m.apply }) }) }));
import { restoreRepoSelectionFromStorage } from './repoSelectionRestore';
import { ApiError } from '../../api/client';
beforeEach(() => {
  vi.clearAllMocks(); m.selected = 'change-one'; m.menu = 'files';
  m.get.mockReset().mockResolvedValue({ id: 'change-one' });
  m.preload.mockReset().mockResolvedValue(true);
  m.apply.mockResolvedValue(undefined);
});

it('starts the saved selection read immediately alongside its context chunk', async () => {
  const restoring = restoreRepoSelectionFromStorage();
  expect(m.get).toHaveBeenCalledWith('change-one');
  expect(m.preload).toHaveBeenCalledOnce();
  await restoring;
  expect(m.apply).toHaveBeenCalledWith({ id: 'change-one' });
});

it('loads nothing when no selection was saved', async () => {
  m.selected = null;
  await restoreRepoSelectionFromStorage();
  expect(m.get).not.toHaveBeenCalled();
  expect(m.preload).not.toHaveBeenCalled();
});

it('retains a selection through chunk failure and offers a working retry', async () => {
  m.preload.mockResolvedValueOnce(false);
  await restoreRepoSelectionFromStorage();
  expect(m.apply).not.toHaveBeenCalled();
  expect(m.selected).toBe('change-one');
  m.toast.mock.calls[0][2].action.onClick();
  await vi.waitFor(() => expect(m.apply).toHaveBeenCalledOnce());
});

it('does not erase or replace a newer selection after an old read finishes', async () => {
  let fail!: (error: Error) => void;
  m.get.mockReturnValueOnce(new Promise((_, reject) => { fail = reject; }));
  const restoring = restoreRepoSelectionFromStorage();
  m.selected = 'change-two';
  fail(new ApiError(404, 'Gone'));
  await restoring;
  expect(m.selected).toBe('change-two');
  expect(m.apply).not.toHaveBeenCalled();
  expect(m.toast).not.toHaveBeenCalled();
});

it('restores the saved context while another panel is active', async () => {
  m.menu = 'apps';
  await restoreRepoSelectionFromStorage();
  expect(m.apply).toHaveBeenCalledWith({ id: 'change-one' });
});

it('clears a definitively deleted selection while another panel is active', async () => {
  m.menu = 'apps';
  m.get.mockRejectedValueOnce(new ApiError(404, 'Gone'));
  await restoreRepoSelectionFromStorage();
  expect(m.selected).toBeNull();
  expect(m.apply).not.toHaveBeenCalled();
});
