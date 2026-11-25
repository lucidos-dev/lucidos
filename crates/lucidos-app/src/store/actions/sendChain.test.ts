import { describe, expect, it, vi } from 'vitest';

vi.mock('../pageOwner', () => ({ pageOwnerId: 'synthetic-page' }));
import { enterSendChain } from './sendChain';

const nextTick = () => new Promise<void>((resolve) => setTimeout(resolve, 0));

describe('send chain ordering', () => {
  it('numbers inputs in the order the sends are entered, not the order they are read', () => {
    const first = enterSendChain('numbered-on-entry');
    const second = enterSendChain('numbered-on-entry');
    expect(second.position.input_seq).toBe(2);
    expect(first.position.input_seq).toBe(1);
    first.release();
    second.release();
  });

  it('keeps the input sequence rising after its chain has drained', async () => {
    const first = enterSendChain('sequence-survives-drain');
    first.release();
    await nextTick();
    const next = enterSendChain('sequence-survives-drain');
    expect(next.position.input_seq).toBe(first.position.input_seq + 1);
    next.release();
  });

  it('a drained chain leaves a send queued behind it in its slot', async () => {
    const first = enterSendChain('successor-keeps-slot');
    const second = enterSendChain('successor-keeps-slot');
    first.release();
    await nextTick();
    const third = enterSendChain('successor-keeps-slot');
    expect(third.waitForTurn).not.toBeNull();
    second.release();
    third.release();
  });
});
