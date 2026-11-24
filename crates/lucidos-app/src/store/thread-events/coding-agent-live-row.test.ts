import { describe, it, expect } from 'vitest';
import { exchangeResponseEvents, exchangeSteps } from './exchange-render';
import type { Exchange } from './exchange';
import type { ResponseEvent, StepOutcome } from '../types';


const TS = '2026-08-11T09:00:00Z';

let seq = 0;
const ev = (event: Record<string, unknown>) => ({ seq: ++seq, event: { created: TS, ...event } as never });

function ccExchange(steps: Exchange['steps'], extra: Partial<Exchange> = {}): Exchange {
  return {
    userEvent: { type: 'MessageReceived', text: 'fix the projection', created: TS, channel: 'claude_code', _eventId: 'msg-1' } as never,
    userSeq: 0,
    steps,
    ...extra,
  };
}

type Row = { description: string; outcome: StepOutcome };

function bothProjections(steps: Exchange['steps'], threadIdle = false, extra: Partial<Exchange> = {}, isLast = true): Record<string, Row[]> {
  const exchange = ccExchange(steps, extra);
  const summary: Row[] = exchangeSteps(exchange, isLast, threadIdle)
    .map(s => ({ description: s.description, outcome: s.outcome }));
  const inline: Row[] = exchangeResponseEvents(exchange, isLast, threadIdle)
    .filter((e): e is Extract<ResponseEvent, { type: 'step' }> => e.type === 'step')
    .map(s => ({ description: s.description, outcome: s.outcome }));
  return { exchangeSteps: summary, exchangeResponseEvents: inline };
}

const pendingOf = (rows: Row[]) => rows.filter(r => r.outcome === 'pending');
const lastOf = <T>(items: T[]): T | undefined => items[items.length - 1];

function forBoth(steps: Exchange['steps'], assert: (rows: Row[], projection: string) => void, threadIdle = false, extra: Partial<Exchange> = {}, isLast = true) {
  for (const [projection, rows] of Object.entries(bothProjections(steps, threadIdle, extra, isLast))) {
    assert(rows, projection);
  }
}

const promptSent = () => ev({ type: 'CodingAgentPromptSent', text: 'fix the projection' });
const toolCalled = (id: string, path: string) =>
  ev({ type: 'CodingAgentToolCalled', name: 'Read', args: { file_path: path }, tool_use_id: id });
const toolResult = (id: string) =>
  ev({ type: 'CodingAgentToolResult', name: 'Read', result: 'ok', tool_use_id: id });
const text = (t: string) => ev({ type: 'CodingAgentTextStreamed', text: t });

describe('a live coding-agent turn always has exactly one pending row', () => {
  it('holds across tool called, tool result, tool called', () => {
    const drive = [promptSent(), toolCalled('tu-a', 'a.rs'), toolResult('tu-a'), toolCalled('tu-b', 'b.rs')];
    for (let n = 1; n <= drive.length; n++) {
      forBoth(drive.slice(0, n), (rows, projection) => {
        expect(pendingOf(rows), `${projection} after ${n} events: ${JSON.stringify(rows)}`).toHaveLength(1);
      });
    }
  });

  it('the row between a result and the next call is a Thinking row', () => {
    forBoth([promptSent(), toolCalled('tu-a', 'a.rs'), toolResult('tu-a')], (rows, projection) => {
      expect(pendingOf(rows), projection).toEqual([{ description: 'Thinking', outcome: 'pending' }]);
      expect(rows.filter(r => r.outcome === 'success'), projection).toHaveLength(1);
    });
  });

  it('the next tool call consumes the row instead of queueing under it', () => {
    forBoth([promptSent(), toolCalled('tu-a', 'a.rs'), toolResult('tu-a'), toolCalled('tu-b', 'b.rs')], (rows, projection) => {
      expect(rows.map(r => r.outcome), projection).toEqual(['success', 'pending']);
      expect(rows[1].description, projection).not.toBe('Thinking');
    });
  });

  it('the row is the FIRST thing a turn shows, before it has produced anything', () => {
    forBoth([
      ev({ type: 'SessionStarted', session_id: 's1', channel: 'claude_code' }),
      ev({ type: 'CodingAgentSettingsChanged', model: 'claude-opus-5[1m]' }),
      ev({ type: 'CodingAgentSettingsChanged', model: 'claude-opus-5[1m]', cc_session_id: 's1' }),
    ], (rows, projection) => {
      expect(rows, projection).toEqual([{ description: 'Thinking', outcome: 'pending' }]);
    });
  });

  it('the first tool call of that turn consumes the row rather than queueing under it', () => {
    forBoth([
      ev({ type: 'SessionStarted', session_id: 's1', channel: 'claude_code' }),
      toolCalled('tu-a', 'a.rs'),
    ], (rows, projection) => {
      expect(rows.map(r => r.outcome), projection).toEqual(['pending']);
      expect(rows[0].description, projection).not.toBe('Thinking');
    });
  });

  it('a resumed session with no CodingAgentPromptSent gets the row too', () => {
    forBoth([toolCalled('tu-a', 'a.rs'), toolResult('tu-a')], (rows, projection) => {
      expect(pendingOf(rows), projection).toEqual([{ description: 'Thinking', outcome: 'pending' }]);
    });
  });
});

describe('parallel tool calls do not each open a row', () => {
  const calls = [promptSent(), toolCalled('tu-a', 'a.rs'), toolCalled('tu-b', 'b.rs'), toolCalled('tu-c', 'c.rs')];

  it('three calls, three staggered results: no Thinking row until the last lands', () => {
    const staggered = [toolResult('tu-a'), toolResult('tu-b')];
    for (let n = 0; n <= staggered.length; n++) {
      forBoth([...calls, ...staggered.slice(0, n)], (rows, projection) => {
        expect(pendingOf(rows), `${projection} with ${n} results in`).toHaveLength(3 - n);
        expect(rows.some(r => r.description === 'Thinking'), `${projection} with ${n} results in`).toBe(false);
      });
    }

    forBoth([...calls, ...staggered, toolResult('tu-c')], (rows, projection) => {
      expect(pendingOf(rows), projection).toEqual([{ description: 'Thinking', outcome: 'pending' }]);
    });
  });
});

describe('a finished turn gains no trailing row', () => {
  const worked = [promptSent(), toolCalled('tu-a', 'a.rs'), toolResult('tu-a'), text('All done.')];

  it('a clean end (CodingAgentIdled) leaves nothing pending', () => {
    forBoth([...worked, ev({ type: 'CodingAgentIdled', has_changes: false })], (rows, projection) => {
      expect(pendingOf(rows), projection).toHaveLength(0);
      expect(lastOf(rows)?.description, projection).not.toBe('Thinking');
    });
  });

  it('an unclean end (ResponseAborted) leaves nothing pending', () => {
    forBoth([...worked, ev({ type: 'ResponseAborted' })], (rows, projection) => {
      expect(pendingOf(rows), projection).toHaveLength(0);
      expect(lastOf(rows)?.description, projection).not.toBe('Thinking');
    });
  });

  it('a quiescent thread with no terminator leaves nothing pending', () => {
    forBoth([promptSent(), toolCalled('tu-a', 'a.rs'), toolResult('tu-a')], (rows, projection) => {
      expect(pendingOf(rows), projection).toHaveLength(0);
    },  true);
  });

  it('a handed-off exchange shimmers nothing', () => {
    forBoth([promptSent(), toolCalled('tu-a', 'a.rs'), toolResult('tu-a')], (rows, projection) => {
      expect(pendingOf(rows), projection).toHaveLength(0);
    },  false, { continuationMoved: true });
  });

  it('an exchange the turn has moved on from shimmers nothing', () => {
    forBoth([promptSent(), toolCalled('tu-a', 'a.rs'), toolResult('tu-a')], (rows, projection) => {
      expect(pendingOf(rows), projection).toHaveLength(0);
    },  false, {},  false);
  });

  it('a switch-teardown boundary shows nothing live, drain or not', () => {
    const teardown: Exchange = {
      userEvent: { type: 'ResponseAborted', cause: 'engine_shutdown', actor: { kind: 'device' }, created: TS, _eventId: 'ab-1' } as never,
      userSeq: 0,
      steps: [
        ev({ type: 'CodingAgentToolResult', name: '', result: "The user doesn't want to proceed with this tool use." }),
        text('\n\n'),
      ],
    };
    expect(exchangeSteps(teardown, true, false).filter(s => s.outcome === 'pending')).toHaveLength(0);
    expect(exchangeResponseEvents(teardown, true, false).filter(e => e.type === 'step')).toHaveLength(0);
  });

  it('a finished turn renders exactly the rows it does today', () => {
    forBoth([...worked, ev({ type: 'CodingAgentIdled', has_changes: false })], (rows, projection) => {
      expect(rows, projection).toEqual([
        { description: 'Read a.rs', outcome: 'success' },
      ]);
    });
  });
});

describe('the live row never splits the response text', () => {
  const streamed = [
    promptSent(),
    toolCalled('tu-a', 'a.rs'),
    toolResult('tu-a'),
    text('Here is the fix:\n\n'),
    text('```ts\nconst x = 1;\n```\n'),
  ];

  it('adjacent visible text stays one text event', () => {
    const events = exchangeResponseEvents(ccExchange(streamed), true, false);
    const texts = events.filter(e => e.type === 'text');
    expect(texts).toHaveLength(1);
    expect((texts[0] as Extract<ResponseEvent, { type: 'text' }>).md).toBe('Here is the fix:\n\n```ts\nconst x = 1;\n```\n');
  });

  it('the live row sits after the prose, as the last row', () => {
    const events = exchangeResponseEvents(ccExchange(streamed), true, false);
    const last = events[events.length - 1];
    expect(last?.type).toBe('step');
    expect(last).toMatchObject({ description: 'Thinking', outcome: 'pending' });
  });
});

describe('only coding-agent turns derive a row', () => {
  it('a live chat turn between a tool result and the next thought is unchanged', () => {
    const chat: Exchange = {
      userEvent: { type: 'MessageReceived', text: 'dedup my list', created: TS, channel: 'chat', _eventId: 'msg-1' } as never,
      userSeq: 0,
      steps: [
        ev({ type: 'ThoughtStreamed' }),
        ev({ type: 'ToolCalled', name: 'read_file', args: { path: 'list.md' } }),
        ev({ type: 'ToolResult', name: 'read_file', result: 'ok' }),
      ],
    };
    const summary = exchangeSteps(chat, true, false);
    expect(summary.map(s => s.outcome)).toEqual(['success']);
    const inline = exchangeResponseEvents(chat, true, false).filter(e => e.type === 'step');
    expect(inline.map(s => (s as Extract<ResponseEvent, { type: 'step' }>).outcome)).toEqual(['success']);
  });

  it('a stepless user message projects no rows at all', () => {
    forBoth([], (rows, projection) => {
      expect(rows, projection).toHaveLength(0);
    });
  });
});
