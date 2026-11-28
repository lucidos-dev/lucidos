import { describe, it, expect } from 'vitest';
import { hasRenderableResponseContent } from '../thread-events';
import type { ResponseEvent } from '../types';

const TS = '2026-08-06T09:16:53Z';

describe('hasRenderableResponseContent', () => {
  it('is false for nothing at all', () => {
    expect(hasRenderableResponseContent([])).toBe(false);
  });

  it('is false for the dying subprocess\'s whitespace flush', () => {
    expect(hasRenderableResponseContent([{ type: 'text', md: '\n\n' }])).toBe(false);
    expect(hasRenderableResponseContent([{ type: 'text', md: '' }])).toBe(false);
    expect(hasRenderableResponseContent([{ type: 'text', md: '   \t \n ' }])).toBe(false);
    expect(hasRenderableResponseContent([{ type: 'text' } as ResponseEvent])).toBe(false);
  });

  it('is true for text with anything in it', () => {
    expect(hasRenderableResponseContent([{ type: 'text', md: 'Merged main.' }])).toBe(true);
  });

  it('is true for every non-text event kind', () => {
    const kinds: ResponseEvent[] = [
      { type: 'step', description: 'Bash', outcome: 'success', created: TS },
      { type: 'image', base64: 'x', mime_type: 'image/png' },
      { type: 'checkpoint', checkpoint_id: 'c1', command: 'rm -rf x', summary: 'x', reverted: false, restores: 1, removes: 0 },
      {
        type: 'event_wait',
        wait_id: 'w1',
        subscriptions: [{ event_type: 'E2ETestsFailed' }],
        reason: 'waiting for the suite',
        created: TS,
        expires_at: TS,
        state: 'waiting',
      },
      { type: 'empty' },
      { type: 'section_break', channel: 'claude_code' },
    ];
    for (const event of kinds) {
      expect(hasRenderableResponseContent([event]), event.type).toBe(true);
    }
  });

  it('a single drawable event rescues a list of blanks', () => {
    expect(hasRenderableResponseContent([
      { type: 'text', md: '\n\n' },
      { type: 'text', md: '  ' },
      { type: 'step', description: 'Bash', outcome: 'success', created: TS },
    ])).toBe(true);
  });
});
