import { describe, expect, it } from 'vitest';
import { threadEmptyStateBody, emptyReason, type EmptyReason } from '../ThreadView';
import { vnodeToText } from './vnodeToText';
import { statusLabel, type ExchangeStatus } from '../../../store/exchange-status';

const BY_KIND: Record<EmptyReason['kind'], EmptyReason> = {
  loading: { kind: 'loading', threadId: 't' },
  animating: { kind: 'animating' },
  failed: { kind: 'failed', threadId: 't' },
  corrupt: { kind: 'corrupt', threadId: 't' },
  disconnected: { kind: 'disconnected', threadId: 't' },
  working: { kind: 'working' },
  empty: { kind: 'empty' },
};
const EVERY_REASON: EmptyReason[] = Object.values(BY_KIND);

const DRAWS_NOTHING: ReadonlySet<EmptyReason['kind']> = new Set(['animating', 'loading']);

describe('every reason a transcript has no exchanges', () => {
  for (const reason of EVERY_REASON) {
    it(`says something for "${reason.kind}"`, () => {
      const drawn = vnodeToText(threadEmptyStateBody(reason,  false));
      if (DRAWS_NOTHING.has(reason.kind)) return;
      expect(drawn.replace(/<[^>]*>/g, '').trim()).not.toBe('');
    });
  }

  it('lists each variant under its own key', () => {
    for (const [kind, reason] of Object.entries(BY_KIND)) {
      expect(reason.kind).toBe(kind);
    }
  });
});

describe('a loaded thread with nothing to draw', () => {
  const loaded = (turnInFlight: boolean): EmptyReason =>
    emptyReason(false, true, false, false, 't', false, turnInFlight);

  it('shows the working indicator while a turn runs', () => {
    expect(loaded(true)).toEqual({ kind: 'working' });
  });

  it('still says it is empty when nothing is running', () => {
    expect(loaded(false)).toEqual({ kind: 'empty' });
  });

  it('lets a failed load win over a turn in flight', () => {
    expect(emptyReason(false, false, true, false, 't', false, true).kind).toBe('failed');
  });

  it('lets a corrupt fold win over a turn in flight', () => {
    expect(emptyReason(false, true, false, true, 't', false, true).kind).toBe('corrupt');
  });
});

describe('every state a turn can be in', () => {
  const EVERY_STATUS: ExchangeStatus[] = [
    'pending', 'queued', 'held', 'streaming', 'coding-agent-working', 'awaiting-answer',
    'done', 'interrupted', 'canceled', 'error', 'aborted',
  ];

  for (const status of EVERY_STATUS) {
    for (const hasSteps of [true, false]) {
      it(`labels "${status}"${hasSteps ? ' with steps' : ''}`, () => {
        const { label, className } = statusLabel(status, hasSteps);
        expect(label).not.toBe('');
        expect(className).not.toBe('');
      });
    }
  }
});
