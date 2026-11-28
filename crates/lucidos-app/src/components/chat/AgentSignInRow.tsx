import { Fragment } from 'preact';
import type { AgentSignInTarget, FormRequestOutcome, ThreadEvent } from '../../generated/thread-event-wire';
import { eventRowBody, type EventRowTone } from './EventRow';

type Notice = Extract<ThreadEvent, { type: 'AgentSignInNeeded' }>;
type Request = Extract<ThreadEvent, { type: 'AgentSignInRequested' }>;

function targetLabel(target: AgentSignInTarget): string {
  return target.kind === 'mcp-server' ? `${target.name} MCP server` : 'Claude Code';
}

export function AgentSignInNotice({ notice }: { notice: Notice }) {
  return eventRowBody({
    kind: 'form',
    role: 'agent-sign-in-notice',
    subject: `${targetLabel(notice.target)} needed a sign-in`,
  });
}

export function CodingAgentCommandAnswer({ answer }: {
  answer: Extract<ThreadEvent, { type: 'CodingAgentCommandAnswered' }>;
}) {
  return <>
    {eventRowBody({ kind: 'form', role: 'coding-agent-command-answer', subject: answer.message })}
    {answer.servers?.map(server => <Fragment key={server.name}>
      {eventRowBody({ kind: 'form', role: 'mcp-server-status', subject: server.name, detail: server.status })}
    </Fragment>)}
  </>;
}

function recordedTarget(request: Request): AgentSignInTarget | null {
  try {
    const { target } = JSON.parse(request.payload);
    if (target?.kind === 'model-login') return target;
    if (target?.kind === 'mcp-server' && typeof target.name === 'string') return target;
    return null;
  } catch {
    return null;
  }
}

export function AgentSignInRequest({ request, outcome, state }: {
  request: Request;
  outcome?: FormRequestOutcome;
  state: { label: string; tone: EventRowTone };
}) {
  const target = recordedTarget(request);
  return eventRowBody({
    kind: 'form',
    role: 'form-request-row',
    state: outcome ?? 'ended',
    subject: target ? `Sign in to ${targetLabel(target)}` : 'Coding agent sign-in',
    stateLabel: outcome ? state.label : 'Ended',
    tone: outcome ? state.tone : 'lapsed',
  });
}
