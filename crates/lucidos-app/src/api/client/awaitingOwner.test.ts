import { describe, expect, it, vi } from 'vitest';
import { AWAITING_OWNER, AWAITING_OWNER_RETRY_MS, untilOwnerAllows } from './awaitingOwner';

describe('untilOwnerAllows', () => {
  it('answers at once when nothing waits', async () => {
    const onWaiting = vi.fn();
    const pause = vi.fn(async () => {});
    await expect(untilOwnerAllows(async () => 'paired', onWaiting, undefined, pause)).resolves.toBe('paired');
    expect(onWaiting).not.toHaveBeenCalled();
    expect(pause).not.toHaveBeenCalled();
  });

  it('presents the secret again after each wait, until it pairs', async () => {
    const answers = [AWAITING_OWNER, AWAITING_OWNER, 'paired'] as const;
    let call = 0;
    const onWaiting = vi.fn();
    const pause = vi.fn(async () => {});
    const result = await untilOwnerAllows(async () => answers[call++], onWaiting, undefined, pause);
    expect(result).toBe('paired');
    expect(call).toBe(3);
    expect(onWaiting).toHaveBeenCalledTimes(2);
    expect(pause).toHaveBeenCalledWith(AWAITING_OWNER_RETRY_MS);
  });

  it('passes a refusal through, as Don\'t allow or an expiry answers', async () => {
    let call = 0;
    const attempt = async () => {
      call += 1;
      if (call === 1) return AWAITING_OWNER;
      throw new Error('the owner declined this pairing on a paired device');
    };
    await expect(untilOwnerAllows(attempt, () => {}, undefined, async () => {})).rejects.toThrow('declined');
  });

  it('stops at the next pause once aborted', async () => {
    const controller = new AbortController();
    const attempt = vi.fn(async () => AWAITING_OWNER);
    const pause = async () => { controller.abort(); };
    await expect(untilOwnerAllows(attempt, () => {}, controller.signal, pause)).rejects.toThrow('stopped waiting');
    expect(attempt).toHaveBeenCalledTimes(1);
  });
});
