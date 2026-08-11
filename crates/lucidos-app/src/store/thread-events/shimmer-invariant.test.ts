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

function ccExchange(steps: Exchange['steps']): Exchange {
  return {
    userEvent: { type: 'MessageReceived', text: 'fix the projection', created: TS, channel: 'claude_code', _eventId: 'msg-1' } as any,
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
  const events = exchangeResponseEvents(exchange, true, false);
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
    stepShimmers: showSteps && !collapsed && events.some(e => e.type === 'step' && e.outcome === 'pending'),
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
  // The gap a coding-agent turn used to have: between a `CodingAgentToolResult`
  // and the next `CodingAgentToolCalled` nothing was pending, so the transcript
  // showed a column of finished checks and the ONLY thing saying work was
  // happening was the header label (reported 2026-08-11: "the running state in
  // between reflects in Working label above which is not good"). The derived
  // live row (`needsLiveThinkingRow`) puts it back in the transcript, and the
  // invariant then resolves the same way as for the native arm.
  const ccGap = ccExchange([
    ev({ type: 'SessionStarted', session_id: 's1' }),
    ev({ type: 'CodingAgentPromptSent', text: 'fix the projection' }),
    ev({ type: 'CodingAgentToolCalled', name: 'Read', args: { file_path: 'a.rs' }, tool_use_id: 'tu-A' }),
    ev({ type: 'CodingAgentToolResult', name: 'Read', result: 'ok', tool_use_id: 'tu-A' }),
  ]);

  it('coding-agent gap between tool calls shimmers the step row, not the label', () => {
    const s = shimmerState(ccGap, /*showSteps*/ true);
    expect(s.statusClass).toBe('working');
    expect(s.stepShimmers).toBe(true);
    expect(s.labelShimmers).toBe(false);
  });

  it('coding-agent gap with steps hidden shimmers the label alone', () => {
    const s = shimmerState(ccGap, /*showSteps*/ false);
    expect(s.statusClass).toBe('working');
    expect(s.stepShimmers).toBe(false);
    expect(s.labelShimmers).toBe(true);
  });

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
