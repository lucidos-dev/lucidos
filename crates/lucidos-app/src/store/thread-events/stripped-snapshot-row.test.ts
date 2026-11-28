import { describe, it, expect } from 'vitest';
import { exchangeResponseEvents } from './exchange-render';
import type { Exchange } from './exchange';
import type { ResponseEvent } from '../types';


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
    const [full] = stepsOf([fullCall()]);
    const [stripped] = stepsOf([strippedCall()]);
    expect(stripped.description).toBe(full.description);
    expect(stripped.description).toBe('Edit shell.css');
  });

  it('stamps the marker and the address the modal fetches by', () => {
    const [step] = stepsOf([strippedCall()]);
    expect(step.args_stripped).toBe(true);
    expect(step.call_event_id).toBe('evt-call');
    expect(step.full).toBeUndefined();
  });

  it('leaves a full row unmarked, with its command already resolved', () => {
    const [step] = stepsOf([fullCall()]);
    expect(step.args_stripped).toBeUndefined();
    expect(step.full).toBe('/a/styles/shell.css');
    expect(step.call_event_id).toBe('evt-call');
  });
});

describe('a stripped chat tool call', () => {
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
