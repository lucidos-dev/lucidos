import { describe, it, expect } from 'vitest';
import type { Exchange } from './exchange';
import { exchangeResponseEvents, exchangeStatus } from './exchange-render';
import { statusLabel } from '../exchange-status';
import { rendersLiveStep } from '../event-rendering';

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
 *  and the inline step shimmer.
 *
 *  Takes the exchange, the steps-expanded toggle, and whether the response panel
 *  is collapsed (which hides the steps body). Then `rowInBand`, which stands for
 *  the whole DOM half of the component's answer: a row was marked, it handed up
 *  its element, and `useOnScreenInTranscript` says that element is in the band.
 *  It is where scroll position enters. A rendered row far below the fold is
 *  drawn and NOT seen, and the label owes the reader the shimmer there. */
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
  const liveStepOnScreen = rendersLiveStep(showSteps, collapsed, events) && rowInBand;
  return {
    status,
    statusClass: className,
    labelShimmers: className === 'working' && !liveStepOnScreen,
    // A step shimmer is on screen iff steps are expanded, the panel isn't
    // collapsed (collapse hides the steps body), a pending step renders, AND
    // that row is inside the transcript's visible band.
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

  // A collapsed response panel hides the steps body, so a pending step's shimmer
  // is NOT on screen — the "Working" header must carry the shimmer instead, even
  // with steps globally expanded and a pending step in the data. (Regression:
  // rendersLiveStep ignored the collapse and suppressed the label shimmer,
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

  // Drawn is not seen. A coding-agent turn always carries a live row. So a turn
  // taller than the pane hid its only shimmer below the fold, leaving "Working"
  // plain over a column of finished checks. That is the same complaint the
  // derived row answered for the data gap, arriving the other way round.
  it('coding-agent gap with its live row below the fold shimmers the label', () => {
    const s = shimmerState(ccGap, /*showSteps*/ true, /*collapsed*/ false, '', /*rowInBand*/ false);
    expect(s.statusClass).toBe('working');
    expect(s.stepShimmers).toBe(false);
    expect(s.labelShimmers).toBe(true);
  });

  it('coding-agent gap with steps hidden shimmers the label alone', () => {
    const s = shimmerState(ccGap, /*showSteps*/ false);
    expect(s.statusClass).toBe('working');
    expect(s.stepShimmers).toBe(false);
    expect(s.labelShimmers).toBe(true);
  });

  // The other direction, and the one this invariant must not be read as
  // requiring. A turn held on a permission card is NOT working, so zero
  // shimmers is right there. A `blocked` row must not be counted as the live
  // one. The whole-transcript version is in
  // `store/__tests__/gated-tool-step.test.ts`, which drives the real fold.
  it('a held call is not a live step, so it cannot satisfy the invariant', () => {
    const held = ccExchange([
      ev({ type: 'SessionStarted', session_id: 's1' }),
      ev({ type: 'CodingAgentToolCalled', name: 'Bash', args: { command: 'rm -rf /tmp/x' }, tool_use_id: 'tu-A' }),
    ]);
    held.blockedStepSeqs = new Set(held.steps.map(s => s.seq));
    const events = exchangeResponseEvents(held, true, false);
    expect(events.filter(e => e.type === 'step' && e.outcome === 'blocked')).toHaveLength(1);
    expect(rendersLiveStep(true, false, events)).toBe(false);
    // And no derived Thinking row was added beside it: the held row IS the
    // turn's current row (`needsLiveThinkingRow`'s `anyLive`).
    expect(events.filter(e => e.type === 'step' && e.description === 'Thinking')).toHaveLength(0);
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
