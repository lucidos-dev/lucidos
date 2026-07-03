import { describe, it, expect } from 'vitest';
import type { Exchange } from './exchange';
import { exchangeResponseEvents, exchangeStatus } from './exchange-render';
import { statusLabel } from '../exchange-status';
import { hasVisibleLiveStep } from '../event-rendering';

const TS = '2026-06-17T12:00:00Z';

function chatExchange(steps: Exchange['steps']): Exchange {
  return {
    userEvent: { type: 'MessageReceived', text: 'dedup my list', created: TS, channel: 'chat', _eventId: 'msg-1' } as any,
    userSeq: 1,
    steps,
  };
}

let seq = 1;
const ev = (event: Record<string, unknown>) => ({ seq: ++seq, event: { created: TS, ...event } as any });

/** Mirror of the ChatExchange render decision for the in-thread status label
 *  and the inline step shimmer, given an exchange + the steps-expanded toggle +
 *  whether the response panel is collapsed (which hides the steps body). */
function shimmerState(exchange: Exchange, showSteps: boolean, collapsed = false, streamingBuffer = '') {
  const events = exchangeResponseEvents(exchange, 0, true, false);
  const status = exchangeStatus(exchange, streamingBuffer, true, false, false, false, false);
  const hasSteps = events.some(e => e.type === 'step');
  const { className } = statusLabel(status, hasSteps);
  const liveStepOnScreen = hasVisibleLiveStep(showSteps, collapsed, events);
  return {
    status,
    statusClass: className,
    labelShimmers: className === 'working' && !liveStepOnScreen,
    // A step shimmer is on screen iff steps are expanded, the panel isn't
    // collapsed (collapse hides the steps body), AND a pending step renders.
    stepShimmers: showSteps && !collapsed && events.some(e => e.type === 'step' && e.success === null),
  };
}

describe('exactly-one-shimmer invariant while working', () => {
  const scenarios: Array<{ name: string; steps: Exchange['steps'] }> = [
    { name: 'thinking only', steps: [ev({ type: 'ThoughtStreamed' })] },
    { name: 'tool running', steps: [ev({ type: 'ThoughtStreamed' }), ev({ type: 'ToolCalled', name: 'read_file', args: { path: 'list.md' } })] },
    {
      name: 'gap after a resolved tool (no pending step)',
      steps: [
        ev({ type: 'ThoughtStreamed' }),
        ev({ type: 'ToolCalled', name: 'read_file', args: { path: 'list.md' } }),
        ev({ type: 'ToolResult', result: 'ok' }),
      ],
    },
    {
      name: 'multiple resolved tools, then text, still working',
      steps: [
        ev({ type: 'ThoughtStreamed' }),
        ev({ type: 'ToolCalled', name: 'read_file', args: { path: 'list.md' } }),
        ev({ type: 'ToolResult', result: 'ok' }),
        ev({ type: 'TextStreamed', text: 'Here is the deduped list.' }),
      ],
    },
  ];

  for (const showSteps of [true, false]) {
    for (const { name, steps } of scenarios) {
      it(`${name} (steps ${showSteps ? 'expanded' : 'collapsed'}) shows at least one shimmer`, () => {
        const s = shimmerState(chatExchange(steps), showSteps);
        if (s.statusClass === 'working') {
          expect(s.labelShimmers || s.stepShimmers).toBe(true);
        }
      });
    }
  }

  // A collapsed response panel hides the steps body, so a pending step's shimmer
  // is NOT on screen — the "Working" header must carry the shimmer instead, even
  // with steps globally expanded and a pending step in the data. (Regression:
  // hasVisibleLiveStep ignored the collapse and suppressed the label shimmer,
  // leaving the working turn with no shimmer at all.)
  it('collapsed panel with a pending step still shimmers the Working label', () => {
    const working = chatExchange([
      ev({ type: 'ThoughtStreamed' }),
      ev({ type: 'ToolCalled', name: 'read_file', args: { path: 'list.md' } }),
    ]);
    const s = shimmerState(working, /*showSteps*/ true, /*collapsed*/ true);
    expect(s.statusClass).toBe('working');
    expect(s.stepShimmers).toBe(false); // body hidden by collapse
    expect(s.labelShimmers).toBe(true); // label carries the sole shimmer
  });
});
