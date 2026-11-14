import { describe, expect, it, vi } from 'vitest';

vi.mock('../pageOwner', () => ({ pageOwnerId: 'synthetic-page' }));
import { enterSendChain } from './sendChain';

describe('send intent compose fences', () => {
  it('rebases three rapid sends only through confirmed immediate predecessors', async () => {
    const first = enterSendChain('rapid', 7);
    const second = enterSendChain('rapid', 7);
    const third = enterSendChain('rapid', 7);
    expect(first.epochForPost()).toBe(7);
    first.accepted(8);
    first.release();
    await second.waitForTurn;
    expect(second.epochForPost()).toBe(8);
    second.accepted(9);
    second.release();
    await third.waitForTurn;
    expect(third.epochForPost()).toBe(9);
    third.release();
  });

  it('keeps the original fence when the immediate predecessor failed', async () => {
    const first = enterSendChain('failure', 7);
    const second = enterSendChain('failure', 7);
    first.release();
    await second.waitForTurn;
    expect(second.epochForPost()).toBe(7);
    second.release();
  });

  it('does not bless an intervening saved acceptance with an epoch jump', async () => {
    const first = enterSendChain('jump', 7);
    const second = enterSendChain('jump', 7);
    first.accepted(9);
    first.release();
    await second.waitForTurn;
    expect(second.epochForPost()).toBe(7);
    second.release();
  });

  it('preserves a newer intent rather than rebasing it backwards', async () => {
    const first = enterSendChain('newer', 7);
    const second = enterSendChain('newer', 9);
    first.accepted(8);
    first.release();
    await second.waitForTurn;
    expect(second.epochForPost()).toBe(9);
    second.release();
  });

  it('rebases a follow-up through an unfenced first send', async () => {
    const first = enterSendChain('unfenced', undefined);
    const second = enterSendChain('unfenced', 0);
    expect(first.epochForPost()).toBeUndefined();
    first.accepted(1);
    first.release();
    await second.waitForTurn;
    expect(second.epochForPost()).toBe(1);
    second.release();
  });

  it('an unfenced first send does not bless an older intent', async () => {
    const first = enterSendChain('unfenced-jump', undefined);
    const second = enterSendChain('unfenced-jump', 0);
    first.accepted(2);
    first.release();
    await second.waitForTurn;
    expect(second.epochForPost()).toBe(0);
    second.release();
  });

  it('an ack without a compose epoch grants no rebase', async () => {
    const first = enterSendChain('missing', 7);
    const second = enterSendChain('missing', 7);
    first.accepted(undefined);
    first.release();
    await second.waitForTurn;
    expect(second.epochForPost()).toBe(7);
    second.release();
  });
});
