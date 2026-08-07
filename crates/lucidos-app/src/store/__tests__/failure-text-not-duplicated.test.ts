/**
 * A turn states its failure ONCE.
 *
 * A coding agent that loses its upstream connection reports the error twice: it
 * streams `API Error: …` as ordinary assistant text before exiting, and the
 * engine records that same string as the turn's `ResponseFailed`. The transcript
 * drew both, so the user read one identical sentence as a plain paragraph and
 * again inside the red failure card immediately beneath it (reported 2026-08-07,
 * on a thread whose Claude Code subprocess hit `ConnectionRefused`).
 *
 * The card is the surface that keeps it: it carries the `ResponseFailed`'s own
 * event id, which is what makes a notification deep-link resolve to the failure
 * (see `ExchangeError.eventId`), and `ChatExchange` renders it as a SIBLING of
 * the response panel, so dropping the paragraph can never hide the error.
 *
 * The match is per-chunk and pre-merge on purpose. A coding agent emits the
 * error as its own text chunk, and `mergeAdjacentTextEvents` would otherwise
 * glue it onto whatever real prose preceded it, leaving nothing that compares
 * equal and putting the duplicate back.
 */
import { describe, it, expect } from 'vitest';
import { exchangeResponseEvents, groupIntoExchanges, type ThreadEvent } from '../thread-events';

const API_ERROR = 'API Error: Connection to the API was lost (ConnectionRefused). This is usually temporary, try again.';

/** The rendered paragraphs of the exchange's single response, in order. */
function paragraphs(events: Map<number, ThreadEvent>): string[] {
  const exchanges = groupIntoExchanges(events);
  return exchangeResponseEvents(exchanges[exchanges.length - 1])
    .filter(e => e.type === 'text')
    .map(e => (e as { md?: string }).md ?? '')
    .filter(md => md.trim() !== '');
}

describe('a failure the agent also streamed is not drawn twice', () => {
  // The exact shape of the 2026-08-07 report: Claude Code streams the error as
  // a chunk with a leading blank line, then the engine terminates the turn with
  // the same string.
  it('drops the streamed copy of a coding agent\'s own API error', () => {
    expect(paragraphs(new Map<number, ThreadEvent>([
      [1, { type: 'MessageReceived', text: 'fix the thing' }],
      [2, { type: 'CodingAgentTextStreamed', text: 'Reading the file.' }],
      [3, { type: 'CodingAgentTextStreamed', text: `\n\n${API_ERROR}` }],
      [4, { type: 'ResponseFailed', error: API_ERROR }],
    ]))).toEqual(['Reading the file.']);
  });

  // Pre-merge matching is the whole point: glued onto the prose above it, the
  // error chunk no longer compares equal to anything.
  it('drops it even when real prose was streamed immediately before', () => {
    expect(paragraphs(new Map<number, ThreadEvent>([
      [1, { type: 'MessageReceived', text: 'fix the thing' }],
      [2, { type: 'CodingAgentTextStreamed', text: 'Reading the file.' }],
      [3, { type: 'CodingAgentTextStreamed', text: API_ERROR }],
      [4, { type: 'ResponseFailed', error: API_ERROR }],
    ]))).toEqual(['Reading the file.']);
  });

  // Channel-agnostic: the rule is "do not print the turn's failure twice", and
  // the chat arm builds its text events the same way.
  it('applies to the chat channel too', () => {
    expect(paragraphs(new Map<number, ThreadEvent>([
      [1, { type: 'MessageReceived', text: 'summarize this' }],
      [2, { type: 'TextStreamed', text: API_ERROR }],
      [3, { type: 'ResponseFailed', error: API_ERROR }],
    ]))).toEqual([]);
  });

  // Only the duplicate goes. Prose that merely MENTIONS the failure is the
  // agent talking about it, which is content the user wants.
  it('keeps text that is not the failure verbatim', () => {
    expect(paragraphs(new Map<number, ThreadEvent>([
      [1, { type: 'MessageReceived', text: 'fix the thing' }],
      [2, { type: 'CodingAgentTextStreamed', text: `That run died with: ${API_ERROR}` }],
      [3, { type: 'ResponseFailed', error: API_ERROR }],
    ]))).toEqual([`That run died with: ${API_ERROR}`]);
  });

  // A turn with no failure is untouched, including one whose text happens to
  // look like an error report.
  it('keeps everything when the turn did not fail', () => {
    expect(paragraphs(new Map<number, ThreadEvent>([
      [1, { type: 'MessageReceived', text: 'fix the thing' }],
      [2, { type: 'CodingAgentTextStreamed', text: API_ERROR }],
      [3, { type: 'CodingAgentIdled' }],
    ]))).toEqual([API_ERROR]);
  });
});
