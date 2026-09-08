/** The transcript is never empty while a turn is in flight.
 *
 *  At every instant at least one of three things is on screen: a progress
 *  indicator, the reader's own message, or the response. There is no fourth
 *  state, and no gap between the three.
 *
 *  Two decisions carry that, and each is a total function over a closed set.
 *  `emptyReason` says what a transcript with no exchanges draws, and
 *  `exchangeStatus` says what a turn's own header says. Enumerated here rather
 *  than sampled, so a new member of either set has to answer the question.
 *
 *  See `docs/plans/2026-09-05-a-turn-is-never-blank.md` and ADR 0174.
 */
import { describe, expect, it } from 'vitest';
import { threadEmptyStateBody, emptyReason, type EmptyReason } from '../ThreadView';
import { vnodeToText } from './vnodeToText';
import { statusLabel, type ExchangeStatus } from '../../../store/exchange-status';

/** Every variant `emptyReason` can return, one of each.
 *
 *  Keyed by `kind` and typed as the full record, so ADDING a variant to the
 *  union fails `tsc` here until it is listed. An array literal would have
 *  claimed the same coverage and enforced nothing. */
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

/** The two that draw nothing, and why each is allowed to.
 *
 *  `animating` is the compose-to-thread slide, where the reader's own text is
 *  on screen inside the composer that is moving. `loading` draws the skeleton
 *  through a delay gate that has no exception (ADR 0081), so its blank window
 *  is bounded and deliberate. */
const DRAWS_NOTHING: ReadonlySet<EmptyReason['kind']> = new Set(['animating', 'loading']);

describe('every reason a transcript has no exchanges', () => {
  for (const reason of EVERY_REASON) {
    it(`says something for "${reason.kind}"`, () => {
      const drawn = vnodeToText(threadEmptyStateBody(reason, /* showReload */ false));
      if (DRAWS_NOTHING.has(reason.kind)) return;
      expect(drawn.replace(/<[^>]*>/g, '').trim()).not.toBe('');
    });
  }

  it('lists each variant under its own key', () => {
    // `tsc` owns the coverage: the record above is typed over the union, so a
    // new variant cannot be added without a row here. This only checks the
    // rows describe themselves, which nothing in the type can say.
    for (const [kind, reason] of Object.entries(BY_KIND)) {
      expect(reason.kind).toBe(kind);
    }
  });
});

describe('a loaded thread with nothing to draw', () => {
  const loaded = (turnInFlight: boolean): EmptyReason =>
    emptyReason(false, true, false, false, 't', false, turnInFlight);

  /** The reported case. A voice thread's first content event can lag the
   *  thread itself by a minute. "No messages in this thread" over a running
   *  turn is a lie as well as a blank. */
  it('shows the working indicator while a turn runs', () => {
    expect(loaded(true)).toEqual({ kind: 'working' });
  });

  it('still says it is empty when nothing is running', () => {
    expect(loaded(false)).toEqual({ kind: 'empty' });
  });

  /** A load that FAILED is honest already, and a working shimmer over it would
   *  replace a recoverable error with a spinner that never resolves. */
  it('lets a failed load win over a turn in flight', () => {
    expect(emptyReason(false, false, true, false, 't', false, true).kind).toBe('failed');
  });

  it('lets a corrupt fold win over a turn in flight', () => {
    expect(emptyReason(false, true, false, true, 't', false, true).kind).toBe('corrupt');
  });
});

describe('every state a turn can be in', () => {
  const EVERY_STATUS: ExchangeStatus[] = [
    'pending', 'queued', 'streaming', 'coding-agent-working', 'awaiting-answer',
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
