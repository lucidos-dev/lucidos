// @vitest-environment jsdom
import { render } from 'preact';
import { afterEach, expect, it } from 'vitest';
import type { AgentSignInTarget } from '../../../generated/thread-event-wire';
import { groupIntoExchanges, hasContentEvents } from '../../../store/thread-events/exchange-grouping';
import type { StoredEvent } from '../../../store/thread-events/thread-event-types';
import { AgentSignInNotice, AgentSignInRequest, CodingAgentCommandAnswer } from '../AgentSignInRow';

const host = document.createElement('div');
afterEach(() => render(null, host));

function drawRecorded(target: AgentSignInTarget) {
  render(<AgentSignInRequest state={{ label: 'Waiting', tone: 'live' }} request={{
    type: 'AgentSignInRequested',
    actor: { kind: 'device', device_id: 'owner' },
    request_id: 'fixture',
    payload: JSON.stringify({ target, local: false, device_id: 'owner', url: 'https://auth.example.test/authorize' }),
  }} />, host);
}

it('renders a recorded sign-in request as an ended row with nothing to act on', () => {
  drawRecorded({ kind: 'mcp-server', name: 'fixture' });
  const row = host.querySelector('[data-role="form-request-row"]');
  expect(row?.getAttribute('data-state')).toBe('ended');
  expect(row?.textContent).toBe('Sign in to fixture MCP serverEnded');
  expect(host.querySelector('a, input, button')).toBeNull();
});

it('renders a recorded sign-in notice with no Sign in button', () => {
  render(<AgentSignInNotice notice={{
    type: 'AgentSignInNeeded', agent: 'claude-code', target: { kind: 'model-login' },
  }} />, host);
  expect(host.textContent).toBe('Claude Code needed a sign-in');
  expect(host.querySelector('button')).toBeNull();
});

it('shows the last MCP list as statuses only', () => {
  render(<CodingAgentCommandAnswer answer={{
    type: 'CodingAgentCommandAnswered',
    agent: 'claude-code',
    message: 'Last reported MCP servers',
    servers: [
      { name: 'calendar', status: 'needs-auth', needs_sign_in: true },
      { name: 'search', status: 'connected', needs_sign_in: false },
    ],
  }} />, host);
  const serverRows = host.querySelectorAll('[data-role="mcp-server-status"]');
  expect([...serverRows].map(row => row.textContent)).toEqual(['calendarneeds-auth', 'searchconnected']);
  expect(host.querySelector('button')).toBeNull();
});

it('keeps a command\'s sign-in suggestion with the command', () => {
  const command: StoredEvent = {
    type: 'CodingAgentCommandReceived', text: '/login', agent: 'claude-code', _eventId: 'command',
  };
  const answer: StoredEvent = {
    type: 'CodingAgentCommandAnswered', agent: 'claude-code',
    message: 'Run the sign-in command below in the thread terminal.', request_event_id: 'command',
  };
  const suggestion: StoredEvent = {
    type: 'TerminalCommandSuggested', suggestion_id: 'fixture', command: 'claude auth login',
    label: 'Sign in to Claude Code', request_event_id: 'command',
  };
  const running: StoredEvent = { type: 'MessageReceived', text: 'Working', mode: 'human', _eventId: 'turn' };
  const exchanges = groupIntoExchanges(new Map<number, StoredEvent>([[1, running], [2, command], [3, answer], [4, suggestion]]));
  expect(exchanges.map(exchange => exchange.steps.map(step => step.event))).toEqual([[], [answer, suggestion]]);
});

it('renders a first command and keeps a later command independent of the running turn', () => {
  const command: StoredEvent = {
    type: 'CodingAgentCommandReceived', text: '/mcp', agent: 'claude-code', _eventId: 'command',
  };
  const answer: StoredEvent = {
    type: 'CodingAgentCommandAnswered', agent: 'claude-code',
    message: 'Last reported MCP servers', servers: [], request_event_id: 'command',
  };
  const first = new Map<number, StoredEvent>([[1, command], [2, answer]]);
  expect(hasContentEvents(first)).toBe(true);
  expect(groupIntoExchanges(first)[0].steps).toEqual([{ seq: 2, event: answer }]);

  const running: StoredEvent = { type: 'MessageReceived', text: 'Working', mode: 'human', _eventId: 'turn' };
  const done: StoredEvent = { type: 'CodingAgentTextStreamed', text: 'Still here' };
  const exchanges = groupIntoExchanges(new Map<number, StoredEvent>([[1, running], [2, command], [3, answer], [4, done]]));
  expect(exchanges.map(exchange => exchange.steps.map(step => step.event))).toEqual([[done], [answer]]);
});
