import { describe, it, expect } from 'vitest';
import type { VNode } from 'preact';
import { actorInitiator, describeExecutor, shouldShowResponseStatusBadge } from './ChatExchange';
import { ClaudeIcon, CodexIcon, PowerIcon, PersonIcon, ApiPlugIcon } from '../shared/icons';
import { LucidosGlyph } from '../shared/LucidosMark';
import { LUCIDOS_AGENT_LABEL, type ThreadEvent } from '../../store/thread-events';

describe('describeExecutor', () => {
  it('shows the SHORT Claude label and the icon for CC threads', () => {
    // "Claude", not "Claude Code": this row is the tightest in the app on a
    // phone and the coding agent's own name was the longest thing in it. Every
    // other "Claude Code" in the app names the backend the user is choosing
    // between and keeps the full product name. See `describeExecutor`.
    const { icon, label } = describeExecutor(true);
    expect(label).toBe('Claude');
    expect((icon as VNode).type).toBe(ClaudeIcon);
  });

  it('shows Codex label and icon for Codex coding-agent threads', () => {
    const { icon, label } = describeExecutor(true, 'codex');
    expect(label).toBe('Codex');
    expect((icon as VNode).type).toBe(CodexIcon);
  });

  it('uses the Codex app mark instead of a red code glyph', () => {
    const icon = CodexIcon() as VNode;
    const props = icon.props as Record<string, unknown>;
    expect(props.stroke).toBe('var(--accent-light)');
    expect(props['stroke-width']).toBe('2.25');
    const group = icon.props.children as VNode;
    expect((group.props as Record<string, unknown>).transform).toBe('translate(-1.2 -1.2) scale(1.1)');
  });

  it('shows Lucidos Agent label + mark glyph for non-CC threads (same entity as the parent_thread initiator label)', () => {
    const { icon, label } = describeExecutor(false);
    expect(label).toBe(LUCIDOS_AGENT_LABEL);
    expect((icon as VNode).type).toBe(LucidosGlyph);
  });
});

describe('actorInitiator (closed set: You / Lucidos Agent / Lucidos Engine / System / API caller)', () => {
  it('device → You (the only origin that is unambiguously the user)', () => {
    const { icon, label } = actorInitiator({ kind: 'device', device_id: 'd', label: 'L' });
    expect(label).toBe('You');
    expect((icon as VNode).type).toBe(PersonIcon);
  });
  it('api with default mode → API caller (anonymous HTTP, never impersonates the user)', () => {
    // Regression: a Lucidos agent that POSTed via raw urllib without forwarding
    // x-lucidos-agent-origin-token used to land as Api{Human} and the chip
    // rendered "You". The chip now refuses to call any non-device origin
    // "You"; the popover still discloses the User-Agent.
    const { icon, label } = actorInitiator({ kind: 'api' });
    expect(label).toBe('API caller');
    expect((icon as VNode).type).toBe(ApiPlugIcon);
  });
  it('api with explicit mode=human → API caller', () => {
    const { icon, label } = actorInitiator({ kind: 'api', mode: 'human', user_agent: 'curl/8' });
    expect(label).toBe('API caller');
    expect((icon as VNode).type).toBe(ApiPlugIcon);
  });
  it('api with mode=agent → Lucidos Agent (mark glyph)', () => {
    const { icon, label } = actorInitiator({ kind: 'api', mode: 'agent' });
    expect(label).toBe(LUCIDOS_AGENT_LABEL);
    expect((icon as VNode).type).toBe(LucidosGlyph);
  });
  it('api with mode=engine → Lucidos Engine (same mark glyph as the agent; label distinguishes)', () => {
    const { icon, label } = actorInitiator({ kind: 'api', mode: 'engine' });
    expect(label).toBe('Lucidos Engine');
    expect((icon as VNode).type).toBe(LucidosGlyph);
  });
  it('workspace with mode=human → API caller (a human in another workspace is not "You" here)', () => {
    const { icon, label } = actorInitiator({ kind: 'workspace', workspace: 'p', mode: 'human' });
    expect(label).toBe('API caller');
    expect((icon as VNode).type).toBe(ApiPlugIcon);
  });
  it('workspace with mode=agent → Lucidos Agent (mark glyph)', () => {
    const { icon, label } = actorInitiator({ kind: 'workspace', workspace: 'p', mode: 'agent' });
    expect(label).toBe(LUCIDOS_AGENT_LABEL);
    expect((icon as VNode).type).toBe(LucidosGlyph);
  });
  it('workspace with mode=engine → Lucidos Engine (mark glyph)', () => {
    const { icon, label } = actorInitiator({ kind: 'workspace', workspace: 'p', mode: 'engine' });
    expect(label).toBe('Lucidos Engine');
    expect((icon as VNode).type).toBe(LucidosGlyph);
  });
  it('parent_thread (default mode=agent) → Lucidos Agent (mark glyph)', () => {
    const { icon, label } = actorInitiator({ kind: 'thread_link', thread_id: 't' });
    expect(label).toBe(LUCIDOS_AGENT_LABEL);
    expect((icon as VNode).type).toBe(LucidosGlyph);
  });
  it('parent_thread with mode=engine → Lucidos Engine (mark glyph)', () => {
    const { icon, label } = actorInitiator({ kind: 'thread_link', thread_id: 't', mode: 'engine' });
    expect(label).toBe('Lucidos Engine');
    expect((icon as VNode).type).toBe(LucidosGlyph);
  });
  it('engine origin → Lucidos Engine (mark glyph)', () => {
    const { icon, label } = actorInitiator({ kind: 'engine', reason: { kind: 'session_recovered' } });
    expect(label).toBe('Lucidos Engine');
    expect((icon as VNode).type).toBe(LucidosGlyph);
  });
  it('system origin → System (power symbol, distinct from the Lucidos mark: the host killed the process, the engine did not choose to)', () => {
    const { icon, label } = actorInitiator({ kind: 'system' });
    expect(label).toBe('System');
    expect((icon as VNode).type).toBe(PowerIcon);
  });
  it('undefined origin → Lucidos Engine (mark glyph)', () => {
    const { icon, label } = actorInitiator(undefined);
    expect(label).toBe('Lucidos Engine');
    expect((icon as VNode).type).toBe(LucidosGlyph);
  });
});

describe('shouldShowResponseStatusBadge', () => {
  const ev = (type: string, rest: Record<string, unknown> = {}) =>
    ({ type, ...rest }) as unknown as ThreadEvent;
  const device = { kind: 'device', device_id: 'd1', label: 'My MacBook' };

  it('hides the canceled badge on a UserQuestionAsked exchange (question card owns the cancel signal)', () => {
    expect(shouldShowResponseStatusBadge(ev('UserQuestionAsked'), 'canceled')).toBe(false);
  });

  it('still shows the canceled badge on a regular MessageReceived exchange', () => {
    expect(shouldShowResponseStatusBadge(ev('MessageReceived'), 'canceled')).toBe(true);
  });

  it('still shows the canceled badge on a CodingAgentPermissionRequest exchange', () => {
    expect(shouldShowResponseStatusBadge(ev('CodingAgentPermissionRequest'), 'canceled')).toBe(true);
  });

  it('shows all non-canceled status badges on a UserQuestionAsked exchange', () => {
    expect(shouldShowResponseStatusBadge(ev('UserQuestionAsked'), 'done')).toBe(true);
    expect(shouldShowResponseStatusBadge(ev('UserQuestionAsked'), 'working')).toBe(true);
    expect(shouldShowResponseStatusBadge(ev('UserQuestionAsked'), 'awaiting')).toBe(true);
    expect(shouldShowResponseStatusBadge(ev('UserQuestionAsked'), 'aborted')).toBe(true);
    expect(shouldShowResponseStatusBadge(ev('UserQuestionAsked'), 'error')).toBe(true);
  });

  /** A "Paused by restart" boundary states its own outcome, and the engine has
   *  promised to resume it. A badge under that panel can only repeat it or
   *  contradict it, so the boundary gets none whatever the status resolves to. */
  it('hides every badge on a switch-teardown boundary', () => {
    const boundary = ev('ResponseAborted', { cause: 'engine_shutdown', actor: device });
    for (const cls of ['aborted', 'done', 'working', 'canceled', 'error']) {
      expect(shouldShowResponseStatusBadge(boundary, cls), cls).toBe(false);
    }
  });

  /** Every other abort boundary keeps its badge: a `safety_net` abort can fire
   *  over a loop that keeps going, and that live turn needs its "Working". */
  it('keeps the badge on an abort boundary the engine did not promise to resume', () => {
    expect(shouldShowResponseStatusBadge(
      ev('ResponseAborted', { cause: 'safety_net', actor: { kind: 'system' } }), 'working',
    )).toBe(true);
    expect(shouldShowResponseStatusBadge(
      ev('ResponseAborted', { cause: 'engine_shutdown', actor: { kind: 'system' } }), 'aborted',
    )).toBe(true);
    expect(shouldShowResponseStatusBadge(
      ev('ResponseAborted', { cause: 'stale_settle', actor: device }), 'aborted',
    )).toBe(true);
  });
});
