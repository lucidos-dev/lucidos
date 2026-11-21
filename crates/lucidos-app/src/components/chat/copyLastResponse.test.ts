import { describe, expect, it } from 'vitest';
import type { Exchange } from '../../store/thread-events';
import { lastResponseText } from './copyLastResponse';

/** An exchange carrying only the streamed text the picker reads. */
function turn(...texts: string[]): Exchange {
  return { steps: texts.map((text) => ({ event: { type: 'TextStreamed', text } })) } as unknown as Exchange;
}

describe('lastResponseText', () => {
  it('copies the newest turn that has a reply', () => {
    expect(lastResponseText([turn('first'), turn('sec', 'ond')])).toBe('second');
  });

  it('skips a newer turn with no reply yet', () => {
    expect(lastResponseText([turn('answer'), turn()])).toBe('answer');
  });

  it('is null when no turn has replied', () => {
    expect(lastResponseText([])).toBeNull();
    expect(lastResponseText([turn('   ')])).toBeNull();
  });
});
