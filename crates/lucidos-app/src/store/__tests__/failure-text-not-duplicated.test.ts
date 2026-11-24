import { describe, it, expect } from 'vitest';
import { exchangeResponseEvents, groupIntoExchanges, type ThreadEvent } from '../thread-events';

const API_ERROR = 'API Error: Connection to the API was lost (ConnectionRefused). This is usually temporary, try again.';

function paragraphs(events: Map<number, ThreadEvent>): string[] {
  const exchanges = groupIntoExchanges(events);
  return exchangeResponseEvents(exchanges[exchanges.length - 1])
    .filter(e => e.type === 'text')
    .map(e => (e as { md?: string }).md ?? '')
    .filter(md => md.trim() !== '');
}

describe('a failure the agent also streamed is not drawn twice', () => {
  it('drops the streamed copy of a coding agent\'s own API error', () => {
    expect(paragraphs(new Map<number, ThreadEvent>([
      [1, { type: 'MessageReceived', text: 'fix the thing' }],
      [2, { type: 'CodingAgentTextStreamed', text: 'Reading the file.' }],
      [3, { type: 'CodingAgentTextStreamed', text: `\n\n${API_ERROR}` }],
      [4, { type: 'ResponseFailed', error: API_ERROR }],
    ]))).toEqual(['Reading the file.']);
  });

  it('drops it even when real prose was streamed immediately before', () => {
    expect(paragraphs(new Map<number, ThreadEvent>([
      [1, { type: 'MessageReceived', text: 'fix the thing' }],
      [2, { type: 'CodingAgentTextStreamed', text: 'Reading the file.' }],
      [3, { type: 'CodingAgentTextStreamed', text: API_ERROR }],
      [4, { type: 'ResponseFailed', error: API_ERROR }],
    ]))).toEqual(['Reading the file.']);
  });

  it('applies to the chat channel too', () => {
    expect(paragraphs(new Map<number, ThreadEvent>([
      [1, { type: 'MessageReceived', text: 'summarize this' }],
      [2, { type: 'TextStreamed', text: API_ERROR }],
      [3, { type: 'ResponseFailed', error: API_ERROR }],
    ]))).toEqual([]);
  });

  it('keeps text that is not the failure verbatim', () => {
    expect(paragraphs(new Map<number, ThreadEvent>([
      [1, { type: 'MessageReceived', text: 'fix the thing' }],
      [2, { type: 'CodingAgentTextStreamed', text: `That run died with: ${API_ERROR}` }],
      [3, { type: 'ResponseFailed', error: API_ERROR }],
    ]))).toEqual([`That run died with: ${API_ERROR}`]);
  });

  it('keeps everything when the turn did not fail', () => {
    expect(paragraphs(new Map<number, ThreadEvent>([
      [1, { type: 'MessageReceived', text: 'fix the thing' }],
      [2, { type: 'CodingAgentTextStreamed', text: API_ERROR }],
      [3, { type: 'CodingAgentIdled' }],
    ]))).toEqual([API_ERROR]);
  });
});
