import { describe, it, expect, beforeEach } from 'vitest';
import { focusedThreadId, wipPreviewThreadId, threadMap, threadsLoaded } from '../store';
import { makeOptimisticThreadState, type ThreadState } from '../thread-events';
import './wipPreview';

const TID = 'wip-thread';

function threadWith(status: ThreadState['meta']['status']): ThreadState {
  const base = makeOptimisticThreadState({
    id: TID,
    title: 'fix it',
    channel: 'claude_code',
    initiator: 'user',
    eventsLoaded: true,
    codingAgentKind: 'app',
    codingAgentFolder: '/data/apps/notes-app',
  });
  return { ...base, meta: { ...base.meta, status } };
}

describe('the WIP preview on a restored page', () => {
  beforeEach(() => {
    wipPreviewThreadId.value = null;
    focusedThreadId.value = null;
    threadMap.value = new Map();
    threadsLoaded.value = false;
  });

  it('keeps restored WIP while the thread list has not loaded, since an empty map is unknown', () => {
    focusedThreadId.value = TID;
    wipPreviewThreadId.value = TID;
    expect(wipPreviewThreadId.value).toBe(TID);
  });

  it('drops the WIP once the loaded list no longer holds its thread', () => {
    focusedThreadId.value = TID;
    wipPreviewThreadId.value = TID;
    threadsLoaded.value = true;
    expect(wipPreviewThreadId.value).toBeNull();
  });
});

describe('the WIP preview through thread changes', () => {
  beforeEach(() => {
    wipPreviewThreadId.value = null;
    focusedThreadId.value = TID;
    threadMap.value = new Map([[TID, threadWith('idle')]]);
    threadsLoaded.value = true;
  });

  it('keeps the WIP through a status change, since only an explicit clear may drop it', () => {
    wipPreviewThreadId.value = TID;
    threadMap.value = new Map([[TID, threadWith('waiting_for_user_answer')]]);
    expect(wipPreviewThreadId.value).toBe(TID);
  });
});
