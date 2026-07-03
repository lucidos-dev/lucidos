// Contract: sendMessage must take ChatContext explicitly via options.context
// rather than reading currentApp / previewFile / selectedLines back from
// view-state signals (frontend.md — Frontend Sends Intent).
import { describe, it, expect, beforeEach, vi } from 'vitest';
import { panelOverlay, threadMap, setFocusedThread } from '../store';
import type { App } from '../types';

vi.mock('../../api/client', async () => {
  const actual = await vi.importActual<typeof import('../../api/client')>('../../api/client');
  return {
    ...actual,
    submitChat: vi.fn().mockResolvedValue({}),
    cancelChat: vi.fn().mockResolvedValue({}),
    stopClaudeCode: vi.fn().mockResolvedValue({}),
    isTransportError: vi.fn().mockReturnValue(false),
  };
});

import { sendMessage } from './chat';
import { submitChat } from '../../api/client';

const testApp: App = { id: 'habit-tracker', name: 'Habit Tracker', description: '' };

describe('sendMessage does not derive context from view-state', () => {
  beforeEach(() => {
    threadMap.value = new Map();
    setFocusedThread(null);
    panelOverlay.value = null;
    vi.clearAllMocks();
  });

  it('omits app_context even when an app-ui panel is open', async () => {
    panelOverlay.value = { type: 'app-ui', app: testApp };

    await sendMessage('hello');

    expect(submitChat).toHaveBeenCalledTimes(1);
    const body = (submitChat as ReturnType<typeof vi.fn>).mock.calls[0][0];
    expect(body.app_context).toBeUndefined();
  });

  it('omits file_context even when a file-preview panel is open', async () => {
    panelOverlay.value = { type: 'file-preview', path: 'notes.md' };

    await sendMessage('hello');

    expect(submitChat).toHaveBeenCalledTimes(1);
    const body = (submitChat as ReturnType<typeof vi.fn>).mock.calls[0][0];
    expect(body.file_context).toBeUndefined();
    expect(body.repo_file_context).toBeUndefined();
  });

  it('forwards explicit app_context passed via options', async () => {
    panelOverlay.value = null;

    await sendMessage('hello', undefined, { context: { app_context: { app_id: 'ledger' } } });

    expect(submitChat).toHaveBeenCalledTimes(1);
    const body = (submitChat as ReturnType<typeof vi.fn>).mock.calls[0][0];
    expect(body.app_context).toEqual({ app_id: 'ledger' });
  });

  it('forwards explicit file_context passed via options', async () => {
    panelOverlay.value = null;

    await sendMessage('hello', undefined, { context: { file_context: { path: 'todo.md' } } });

    expect(submitChat).toHaveBeenCalledTimes(1);
    const body = (submitChat as ReturnType<typeof vi.fn>).mock.calls[0][0];
    expect(body.file_context).toEqual({ path: 'todo.md' });
  });
});
