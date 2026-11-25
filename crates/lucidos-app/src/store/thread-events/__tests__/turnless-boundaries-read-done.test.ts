import { describe, expect, it } from 'vitest';
import { exchangeStatus, type StoredEvent } from '../../thread-events';
import { makeExchange, step } from '../../__tests__/fixtures';

describe('turns that read done while the thread runs', () => {
  it('a placeholder whose announcement carries neither id still reads done while the thread runs', () => {
    const followup = makeExchange({ type: 'MessageReceived', text: 'second' } as StoredEvent, [
      step(1, { type: 'PromptInjected', text: 'second', injected_message_id: 'legacy-followup' } as StoredEvent),
    ]);
    expect(exchangeStatus(followup, '', true, false, false, false)).toBe('done');
  });

  it('a user stop boundary reads done while the thread still runs', () => {
    const stop = makeExchange({ type: 'EventWaitCanceled', wait_id: 'w1', cause: 'user_stop' } as StoredEvent);
    expect(exchangeStatus(stop, '', true, false, false, false)).toBe('done');
  });
});
