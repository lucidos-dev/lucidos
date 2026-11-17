import { afterEach, beforeEach, expect, it, vi } from 'vitest';
const m = vi.hoisted(() => ({ loaded: false, preload: vi.fn(), send: vi.fn() }));
vi.mock('../../utils/lazyComponent', async (importOriginal) => {
  const actual = await importOriginal<typeof import('../../utils/lazyComponent')>();
  return { ...actual, lazyModule: (loader: () => Promise<unknown>) => loader.toString().includes('/chat')
    ? { isLoaded: () => m.loaded, preload: m.preload, get: () => ({ sendMessage: m.send }) }
    : actual.lazyModule(loader) };
});
import { sendMessage } from './chatSend';
import { sendFollowup } from './compose';
import { threadMap, toasts } from '../store';
import { getDraft, setDraft, _resetComposeDraftsForTesting } from '../composeDrafts';
import { makeThreadState } from './threads-test-helpers';

beforeEach(() => {
  vi.useFakeTimers();
  vi.clearAllMocks();
  m.loaded = false;
  m.preload.mockReset().mockResolvedValue(true);
  m.send.mockResolvedValue('sent');
  toasts.value = [];
});
afterEach(() => {
  threadMap.value = new Map();
  _resetComposeDraftsForTesting();
  vi.clearAllTimers();
  vi.useRealTimers();
});

it('forwards the bound send unchanged after loading, and synchronously when warm', async () => {
  const options = { threadId: 'thread-one', useCodingAgent: false };
  const images = ['image-one'];
  expect(await sendMessage('message', images, options)).toBe('sent');
  expect(m.send).toHaveBeenCalledWith('message', images, options);
  expect(m.send.mock.calls[0][2]).toBe(options);
  m.loaded = true;
  const sending = sendMessage('warm', undefined, options);
  expect(m.send).toHaveBeenCalledTimes(2);
  await sending;
});

it('restores an unsent follow-up beside newer edits when the send chunk fails', async () => {
  let finish!: (loaded: boolean) => void;
  m.preload.mockReturnValueOnce(new Promise<boolean>((resolve) => { finish = resolve; }));
  threadMap.value = new Map([['thread-one', makeThreadState('thread-one')]]);
  setDraft('thread-one', { mode: null, text: 'original', image_hashes: ['image-original'] });
  const sending = sendFollowup('thread-one', 'original', ['image-original']);
  expect(getDraft('thread-one').text).toBe('');
  setDraft('thread-one', { mode: null, text: 'newer edit', image_hashes: ['image-new'] });
  finish(false);
  await sending;
  expect(getDraft('thread-one').text).toBe('newer edit\n\noriginal');
  expect(getDraft('thread-one').image_hashes).toEqual(['image-new', 'image-original']);
  expect(m.send).not.toHaveBeenCalled();
  expect(toasts.value.some((toast) => toast.message.includes('sending code'))).toBe(true);
});
