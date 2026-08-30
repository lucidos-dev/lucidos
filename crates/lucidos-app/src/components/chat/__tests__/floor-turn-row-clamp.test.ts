// The oldest turn on screen draws its TAIL, and its head arrives by scrolling.
//
// `threadWindow.test.ts` owns the arithmetic. This owns the two halves that
// live outside it: the memo has to answer to the clamp, and the head must have
// no control of its own.
//
// The second is not a style preference. A per-turn "Show earlier steps"
// expander shipped twice and was removed twice, the second time because the
// user disliked it from the first message. See
// docs/plans/2026-06-26-perf-instrumentation-remove-step-cap.md.

import { describe, it, expect } from 'vitest';
// @ts-expect-error: Node APIs available at runtime via Vitest, no @types/node in project
import { readFileSync, readdirSync } from 'node:fs';
// @ts-expect-error: same
import { fileURLToPath } from 'node:url';
// @ts-expect-error: same
import { dirname, join } from 'node:path';
import { chatExchangePropsEqual } from '../ChatExchange';
import type { Exchange } from '../../../store/thread-events';

const STEP = { seq: 7, event: { type: 'CodingAgentToolCalled', created: 'T' } as never };

function exchange(): Exchange {
  return {
    userEvent: { type: 'MessageReceived', text: 'hi', created: 'T', _eventId: 'm1' } as never,
    userSeq: 1,
    steps: [STEP],
  };
}

const props = (rowsHidden: number) => ({ exchange: exchange(), revision: 0, rowsHidden } as never);

describe('the memo answers to the row clamp', () => {
  it('re-renders when a scroll-up round uncovers part of the head', () => {
    // Nothing else about the floor turn changes on a round: same events, same
    // revision, same status. Without this the head never draws and the reader
    // scrolls into a turn that will not open.
    expect(chatExchangePropsEqual(props(200), props(120))).toBe(false);
  });

  it('re-renders when the head is fully uncovered', () => {
    expect(chatExchangePropsEqual(props(40), props(0))).toBe(false);
  });

  it('skips the render when the clamp is unchanged', () => {
    expect(chatExchangePropsEqual(props(80), props(80))).toBe(true);
  });
});

describe('the clamped head has no control of its own', () => {
  const CHAT_DIR = join(dirname(fileURLToPath(import.meta.url)), '..');
  /** The shapes the twice-removed expander took, by class and by copy. */
  const BANNED = [
    'step-window-expander',
    'renderStepExpander',
    'Show earlier steps',
    'EXCHANGE_STEP_CAP',
    'computeStepClamp',
    'expandedExchanges',
  ];

  const sources: string[] = readdirSync(CHAT_DIR)
    .filter((name: string) => (name.endsWith('.tsx') || name.endsWith('.ts')) && !name.includes('.test.'));

  it('scans a real set of transcript sources', () => {
    // A directory read that matched nothing would pass the sweep below in
    // silence, and so would a hardcoded filename that has since been renamed.
    expect(sources).toContain('ChatExchange.tsx');
    expect(sources).toContain('chat-exchange-parts.tsx');
    expect(sources).toContain('threadWindow.ts');
    expect(sources).toContain('ThreadView.tsx');
    expect(sources.length).toBeGreaterThan(20);
  });

  it('reveals the head by scrolling, not by a button', () => {
    for (const name of sources) {
      const source = readFileSync(join(CHAT_DIR, name), 'utf8');
      // The comments that record the ban have to name it, so only CODE counts.
      const code = source.split('\n')
        .filter((line: string) => !/^\s*(\/\/|\*|\/\*)/.test(line))
        .join('\n');
      for (const banned of BANNED) {
        expect(code, `${name} must not reintroduce ${banned}`).not.toContain(banned);
      }
    }
  });
});
