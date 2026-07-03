// Shared scaffolding for the store/actions thread tests, split across
// threads-*.test.ts.
import { type ThreadMeta, type ThreadState } from '../thread-events';
import { setDraft, type ComposeDraft } from '../composeDrafts';

interface MakeThreadOverrides extends Partial<Omit<ThreadState, 'meta'>> {
  meta?: Partial<ThreadMeta> & {
    composeText?: string;
    composeImages?: string[];
    composeMode?: ComposeDraft['mode'];
  };
}

export function makeThreadState(id: string, overrides: MakeThreadOverrides = {}): ThreadState {
  const { composeText, composeImages, composeMode, ...metaOverrides } = overrides.meta ?? {};
  if (composeText !== undefined || composeImages !== undefined || composeMode !== undefined) {
    setDraft(id, {
      text: composeText ?? '',
      image_hashes: composeImages ?? [],
      mode: composeMode ?? null,
    });
  }
  return {
    meta: {
      id,
      title: `Thread ${id}`,
      channel: 'chat',
      initiator: 'user',
      saved: false,
      createdAt: '2026-01-01T00:00:00Z',
      updatedAt: '2026-01-01T00:00:00Z',
      status: 'idle',
      codingAgentProposed: false,
      codingAgentRequiresRestart: false,
      codingAgentIsExternalRepo: false,
      codingAgentApplying: false,
      codingAgentHasDiff: false,
      lastRevivedAt: '',
      messageCount: 0,
      section: 'archived',
      activeChildrenCount: 0,
      totalChildrenCount: 0,
      blockingDescendantCount: 0, attentionDescendantCount: 0,
      state: 'active',
      latestTodoList: null,
      ...metaOverrides,
    },
    events: overrides.events || new Map(),
    streamingBuffer: overrides.streamingBuffer || '',
    eventsLoaded: overrides.eventsLoaded || false,
    eventsLoadFailed: overrides.eventsLoadFailed ?? false,
    lastDbSeq: overrides.lastDbSeq ?? 0,
    pendingUserMessages: overrides.pendingUserMessages || [],
  };
}
