import { describe, it, expect, beforeEach, vi } from 'vitest';

vi.mock('../../api/client', async (importOriginal) => ({
  ...(await importOriginal<typeof import('../../api/client')>()),
  applyNow: vi.fn(),
}));

import { endClaudeCodeAndApply } from './codingAgentChanges';
import { threadMap, applyNowRequests } from '../store';
import { makeThreadState } from './threads-test-helpers';
import { applyNow } from '../../api/client';

const mockApplyNow = vi.mocked(applyNow);

function seedVersion(summaryVersion: number): void {
  threadMap.value = new Map([
    ['t', makeThreadState('t', {
      meta: {
        id: 't', title: 'CC thread', channel: 'claude_code', saved: false, createdAt: '',
        updatedAt: '2026-01-01T00:00:01Z', status: 'waiting',
        codingAgentChangeState: { kind: 'proposed', requires_restart: false },
        codingAgentIsExternalRepo: false, lastRevivedAt: '', messageCount: 1, section: 'inbox',
        activeChildrenCount: 0, summaryVersion,
      },
    })],
  ]);
}

describe('endClaudeCodeAndApply: legacy version bridge', () => {
  beforeEach(() => {
    applyNowRequests.value = new Map();
    mockApplyNow.mockReset();
  });

  it('holds an engine without a summary version until a row newer than the one read before the request', async () => {
    seedVersion(3);
    mockApplyNow.mockImplementation(async () => {
      seedVersion(5);
      return {} as Awaited<ReturnType<typeof applyNow>>;
    });
    await endClaudeCodeAndApply('t');
    expect(applyNowRequests.value.get('t')).toBe(4);
  });
});
