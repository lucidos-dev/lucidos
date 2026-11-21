import { describe, it, expect } from 'vitest';
import { exchangeResponseEvents } from './exchange-render';
import type { Exchange } from './exchange';
import type { ResponseEvent } from '../types';

/** A row arrives with its heavy fields already gone.
 *
 *  The engine strips `CodingAgentToolCalled.args` and
 *  `CodingAgentToolResult.result`, because they were the bulk of a coding-agent
 *  thread's transfer and nothing renders them inline. The snapshot and the live
 *  stream strip alike (`ThreadEvent::strip_tool_call_args`). A full row still
 *  reaches the fold from `include_context`.
 *
 *  So the fold has to serve two shapes of the same event, and this pins what
 *  each owes. A stripped row must read identically in the transcript and must
 *  carry the address the step-detail modal fetches by. A full row must carry
 *  the values themselves and no marker, so the modal never fetches what it
 *  already holds. */

const TS = '2026-08-30T09:00:00Z';

let seq = 0;
const ev = (event: Record<string, unknown>) => ({ seq: ++seq, event: { created: TS, ...event } as never });

function ccExchange(steps: Exchange['steps']): Exchange {
  return {
    userEvent: { type: 'MessageReceived', text: 'edit it', created: TS, channel: 'claude_code', _eventId: 'msg-1' } as never,
    userSeq: 0,
    steps,
  };
}

function stepsOf(steps: Exchange['steps']): Extract<ResponseEvent, { type: 'step' }>[] {
  return exchangeResponseEvents(ccExchange(steps), true, true)
    .filter((e): e is Extract<ResponseEvent, { type: 'step' }> => e.type === 'step');
}

/** The same call, with its args (`include_context`) and stripped. */
const fullCall = () => ev({
  type: 'CodingAgentToolCalled',
  name: 'Edit',
  description: 'Edit shell.css',
  args: { file_path: '/a/styles/shell.css', old_string: 'a', new_string: 'b' },
  tool_use_id: 'tu-A',
  _eventId: 'evt-call',
});
const strippedCall = () => ev({
  type: 'CodingAgentToolCalled',
  name: 'Edit',
  description: 'Edit shell.css',
  args_stripped: true,
  tool_use_id: 'tu-A',
  _eventId: 'evt-call',
});

const fullResult = () => ev({
  type: 'CodingAgentToolResult',
  name: 'Edit',
  result: 'applied 1 hunk',
  tool_use_id: 'tu-A',
  _eventId: 'evt-result',
});
const strippedResult = () => ev({
  type: 'CodingAgentToolResult',
  name: 'Edit',
  result_stripped: true,
  tool_use_id: 'tu-A',
  _eventId: 'evt-result',
});

describe('a stripped coding-agent tool call', () => {
  it('renders the label a full one renders', () => {
    // The strip fills `description` before dropping `args`, so the two rows
    // reach the fold carrying the same label. That equality is the whole
    // safety of the strip: the label is the only thing the transcript draws.
    const [full] = stepsOf([fullCall()]);
    const [stripped] = stepsOf([strippedCall()]);
    expect(stripped.description).toBe(full.description);
    expect(stripped.description).toBe('Edit shell.css');
  });

  it('stamps the marker and the address the modal fetches by', () => {
    const [step] = stepsOf([strippedCall()]);
    expect(step.args_stripped).toBe(true);
    expect(step.call_event_id).toBe('evt-call');
    // Nothing to show inline: the modal loads the un-elided command instead.
    expect(step.full).toBeUndefined();
  });

  it('leaves a full row unmarked, with its command already resolved', () => {
    const [step] = stepsOf([fullCall()]);
    expect(step.args_stripped).toBeUndefined();
    expect(step.full).toBe('/a/styles/shell.css');
    // Still addressed, so a modal opened on it can re-fetch if it ever needs to.
    expect(step.call_event_id).toBe('evt-call');
  });
});

describe('a stripped chat tool call', () => {
  // Every live chat call arrives this way, so the modal must know its channel
  // to format the args it fetches back through the chat describer.
  it('stamps the marker, the address and the chat channel', () => {
    const [step] = stepsOf([ev({
      type: 'ToolCalled',
      name: 'run_bash',
      description: 'Running ls',
      args_stripped: true,
      _eventId: 'evt-chat-call',
    })]);
    expect(step.description).toBe('Running ls');
    expect(step.args_stripped).toBe(true);
    expect(step.tool_channel).toBe('chat');
    expect(step.call_event_id).toBe('evt-chat-call');
    expect(step.full).toBeUndefined();
  });
});

describe('a stripped coding-agent tool result', () => {
  it('settles its step and stamps the marker and address', () => {
    const [step] = stepsOf([strippedCall(), strippedResult()]);
    expect(step.outcome).toBe('success');
    expect(step.result).toBeUndefined();
    expect(step.result_stripped).toBe(true);
    expect(step.result_event_id).toBe('evt-result');
  });

  it('leaves a full row unmarked, with its text already inline', () => {
    const [step] = stepsOf([fullCall(), fullResult()]);
    expect(step.result).toBe('applied 1 hunk');
    expect(step.result_stripped).toBeUndefined();
    expect(step.result_event_id).toBe('evt-result');
  });

  it('addresses the step it settles by fallback, not only by tool_use_id', () => {
    // The pairing normally goes by `tool_use_id`. A result carrying none
    // settles the last pending step instead, and that path has to stamp the
    // same two fields. Otherwise the modal opens on a row it cannot fetch.
    const unpaired = ev({
      type: 'CodingAgentToolResult',
      name: 'Edit',
      result_stripped: true,
      _eventId: 'evt-unpaired',
    });
    const [step] = stepsOf([strippedCall(), unpaired]);
    expect(step.result_stripped).toBe(true);
    expect(step.result_event_id).toBe('evt-unpaired');
  });
});
