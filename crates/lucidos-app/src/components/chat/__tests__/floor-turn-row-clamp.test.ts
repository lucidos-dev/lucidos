
import { describe, it, expect } from 'vitest';
// @ts-expect-error: Node APIs available at runtime via Vitest, no @types/node in project
import { readFileSync, readdirSync } from 'node:fs';
// @ts-expect-error: same
import { fileURLToPath } from 'node:url';
// @ts-expect-error: same
import { dirname, join } from 'node:path';
import { chatExchangePropsEqual } from '../ChatExchange';
import { turnBodyFolded, type Exchange } from '../../../store/thread-events';

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
    expect(chatExchangePropsEqual(props(200), props(120))).toBe(false);
  });

  it('re-renders when the head is fully uncovered', () => {
    expect(chatExchangePropsEqual(props(40), props(0))).toBe(false);
  });

  it('skips the render when the clamp is unchanged', () => {
    expect(chatExchangePropsEqual(props(80), props(80))).toBe(true);
  });
});

describe('the body is chosen by the questions the window asks', () => {
  const source: string = readFileSync(
    join(dirname(fileURLToPath(import.meta.url)), '..', 'ChatExchange.tsx'), 'utf8');

  it('hands the clamp to the body, which picks its path by the shared predicate', () => {
    expect(source).toContain('responseBody(events, { showSteps, showDetails, rowsHidden })');
  });

  it('builds no body for a folded turn, and asks the shared helper whether it is', () => {
    expect(source).toContain('turnBodyFolded(collapsedExchanges.value, threadId, exchange)');
    expect(source).toContain('hasEvents && !bodyFolded ? responseBody(');
  });

  it('reads a fold by thread and turn', () => {
    const folds = new Set(['t1:1']);
    expect(turnBodyFolded(folds, 't1', exchange())).toBe(true);
    expect(turnBodyFolded(folds, 't2', exchange())).toBe(false);
    expect(turnBodyFolded(new Set(), 't1', exchange())).toBe(false);
  });
});

describe('the clamped head has no control of its own', () => {
  const CHAT_DIR = join(dirname(fileURLToPath(import.meta.url)), '..');
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
    expect(sources).toContain('ChatExchange.tsx');
    expect(sources).toContain('chat-exchange-parts.tsx');
    expect(sources).toContain('threadWindow.ts');
    expect(sources).toContain('ThreadView.tsx');
    expect(sources.length).toBeGreaterThan(20);
  });

  it('reveals the head by scrolling, not by a button', () => {
    for (const name of sources) {
      const source = readFileSync(join(CHAT_DIR, name), 'utf8');
      const code = source.split('\n')
        .filter((line: string) => !/^\s*(\/\/|\*|\/\*)/.test(line))
        .join('\n');
      for (const banned of BANNED) {
        expect(code, `${name} must not reintroduce ${banned}`).not.toContain(banned);
      }
    }
  });
});
