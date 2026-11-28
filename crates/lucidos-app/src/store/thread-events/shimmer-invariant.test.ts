import { describe, it, expect } from 'vitest';
import type { Exchange } from './exchange';
import { exchangeResponseEvents, exchangeStatus } from './exchange-render';
import { statusLabel } from '../exchange-status';
import { liveStepInBody, responseBody } from '../event-rendering';

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

function shimmerState(
  exchange: Exchange,
  showSteps: boolean,
  collapsed = false,
  streamingBuffer = '',
  rowInBand = true,
) {
  const events = exchangeResponseEvents(exchange, true, false);
  const status = exchangeStatus(exchange, streamingBuffer, true, false, false, false, false);
  const hasSteps = events.some(e => e.type === 'step');
  const { className } = statusLabel(status, hasSteps);
  const liveRow = collapsed ? -1 : liveStepInBody(responseBody(events, { showSteps, showDetails: true, rowsHidden: 0 }));
  const liveStepOnScreen = liveRow >= 0 && rowInBand;
  return {
    status,
    statusClass: className,
    labelShimmers: className === 'working' && !liveStepOnScreen,
    stepShimmers: showSteps && !collapsed && rowInBand
      && events.some(e => e.type === 'step' && e.outcome === 'pending'),
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
    for (const rowInBand of [true, false]) {
      for (const { name, steps } of scenarios) {
        const where = `steps ${showSteps ? 'expanded' : 'collapsed'}, row ${rowInBand ? 'in band' : 'below the fold'}`;
        it(`${name} (${where}) shows at least one shimmer`, () => {
          const s = shimmerState(chatExchange(steps), showSteps, false, '', rowInBand);
          if (s.statusClass === 'working') {
            expect(s.labelShimmers || s.stepShimmers).toBe(true);
          }
        });
      }
    }
  }

  const ccGap = ccExchange([
    ev({ type: 'SessionStarted', session_id: 's1' }),
    ev({ type: 'CodingAgentPromptSent', text: 'fix the projection' }),
    ev({ type: 'CodingAgentToolCalled', name: 'Read', args: { file_path: 'a.rs' }, tool_use_id: 'tu-A' }),
    ev({ type: 'CodingAgentToolResult', name: 'Read', result: 'ok', tool_use_id: 'tu-A' }),
  ]);

  it('coding-agent gap between tool calls shimmers the step row, not the label', () => {
    const s = shimmerState(ccGap,  true);
    expect(s.statusClass).toBe('working');
    expect(s.stepShimmers).toBe(true);
    expect(s.labelShimmers).toBe(false);
  });

  it('coding-agent gap with its live row below the fold shimmers the label', () => {
    const s = shimmerState(ccGap,  true,  false, '',  false);
    expect(s.statusClass).toBe('working');
    expect(s.stepShimmers).toBe(false);
    expect(s.labelShimmers).toBe(true);
  });

  it('coding-agent gap with steps hidden shimmers the label alone', () => {
    const s = shimmerState(ccGap,  false);
    expect(s.statusClass).toBe('working');
    expect(s.stepShimmers).toBe(false);
    expect(s.labelShimmers).toBe(true);
  });

  it('a held call is not a live step, so it cannot satisfy the invariant', () => {
    const held = ccExchange([
      ev({ type: 'SessionStarted', session_id: 's1' }),
      ev({ type: 'CodingAgentToolCalled', name: 'Bash', args: { command: 'rm -rf /tmp/x' }, tool_use_id: 'tu-A' }),
    ]);
    held.blockedStepSeqs = new Set(held.steps.map(s => s.seq));
    const events = exchangeResponseEvents(held, true, false);
    expect(events.filter(e => e.type === 'step' && e.outcome === 'blocked')).toHaveLength(1);
    expect(liveStepInBody(responseBody(events, { showSteps: true, showDetails: true, rowsHidden: 0 }))).toBe(-1);
    expect(events.filter(e => e.type === 'step' && e.description === 'Thinking')).toHaveLength(0);
  });

  it('collapsed panel with a pending step still shimmers the Working label', () => {
    const working = chatExchange([
      ev({ type: 'ThoughtStreamed' }),
      ev({ type: 'ToolCalled', name: 'read_file', args: { path: 'list.md' } }),
    ]);
    const s = shimmerState(working,  true,  true);
    expect(s.statusClass).toBe('working');
    expect(s.stepShimmers).toBe(false);
    expect(s.labelShimmers).toBe(true);
  });
});
