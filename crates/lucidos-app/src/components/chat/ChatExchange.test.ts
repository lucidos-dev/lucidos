import { describe, it, expect } from 'vitest';
import type { VNode } from 'preact';
import { actorInitiator, describeExecutor, shouldShowResponseStatusBadge } from './ChatExchange';
import { ClaudeIcon, CodexIcon } from '../shared/icons';
import { LucidosAgentGlyph } from '../shared/LucidosMark';
import { LUCIDOS_AGENT_LABEL } from '../../store/thread-events';

describe('describeExecutor', () => {
  it('shows Claude Code label and icon for CC threads', () => {
    const { icon, label } = describeExecutor(true);
    expect(label).toBe('Claude Code');
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
    expect((icon as VNode).type).toBe(LucidosAgentGlyph);
  });
});

describe('actorInitiator (closed set: You / Lucidos Agent / Lucidos Engine / System / API caller)', () => {
  it('device → You (the only origin that is unambiguously the user)', () => {
    expect(actorInitiator({ kind: 'device', device_id: 'd', label: 'L' }))
      .toEqual({ icon: '\u{1F464}', label: 'You' });
  });
  it('api with default mode → API caller (anonymous HTTP, never impersonates the user)', () => {
    // Regression: a Lucidos agent that POSTed via raw urllib without forwarding
    // x-lucidos-agent-origin-token used to land as Api{Human} and the chip
    // rendered "You". The chip now refuses to call any non-device origin
    // "You"; the popover still discloses the User-Agent.
    expect(actorInitiator({ kind: 'api' })).toEqual({ icon: '🔌', label: 'API caller' });
  });
  it('api with explicit mode=human → API caller', () => {
    expect(actorInitiator({ kind: 'api', mode: 'human', user_agent: 'curl/8' }))
      .toEqual({ icon: '🔌', label: 'API caller' });
  });
  it('api with mode=agent → Lucidos Agent (mark glyph)', () => {
    const { icon, label } = actorInitiator({ kind: 'api', mode: 'agent' });
    expect(label).toBe(LUCIDOS_AGENT_LABEL);
    expect((icon as VNode).type).toBe(LucidosAgentGlyph);
  });
  it('api with mode=engine → Lucidos Engine', () => {
    expect(actorInitiator({ kind: 'api', mode: 'engine' }))
      .toEqual({ icon: '⬡', label: 'Lucidos Engine' });
  });
  it('workspace with mode=human → API caller (a human in another workspace is not "You" here)', () => {
    expect(actorInitiator({ kind: 'workspace', workspace: 'p', mode: 'human' }))
      .toEqual({ icon: '🔌', label: 'API caller' });
  });
  it('workspace with mode=agent → Lucidos Agent (mark glyph)', () => {
    const { icon, label } = actorInitiator({ kind: 'workspace', workspace: 'p', mode: 'agent' });
    expect(label).toBe(LUCIDOS_AGENT_LABEL);
    expect((icon as VNode).type).toBe(LucidosAgentGlyph);
  });
  it('workspace with mode=engine → Lucidos Engine', () => {
    expect(actorInitiator({ kind: 'workspace', workspace: 'p', mode: 'engine' }))
      .toEqual({ icon: '⬡', label: 'Lucidos Engine' });
  });
  it('parent_thread (default mode=agent) → Lucidos Agent (mark glyph)', () => {
    const { icon, label } = actorInitiator({ kind: 'thread_link', thread_id: 't' });
    expect(label).toBe(LUCIDOS_AGENT_LABEL);
    expect((icon as VNode).type).toBe(LucidosAgentGlyph);
  });
  it('parent_thread with mode=engine → Lucidos Engine', () => {
    expect(actorInitiator({ kind: 'thread_link', thread_id: 't', mode: 'engine' }))
      .toEqual({ icon: '⬡', label: 'Lucidos Engine' });
  });
  it('engine origin → Lucidos Engine', () => {
    expect(actorInitiator({ kind: 'engine', reason: { kind: 'session_recovered' } }))
      .toEqual({ icon: '⬡', label: 'Lucidos Engine' });
  });
  it('system origin → System (distinct from engine — process killed by host, not engine-deliberate)', () => {
    expect(actorInitiator({ kind: 'system' })).toEqual({ icon: '⚙', label: 'System' });
  });
  it('undefined origin → Lucidos Engine', () => {
    expect(actorInitiator(undefined)).toEqual({ icon: '⬡', label: 'Lucidos Engine' });
  });
});

describe('shouldShowResponseStatusBadge', () => {
  it('hides the canceled badge on a UserQuestionAsked exchange (question card owns the cancel signal)', () => {
    expect(shouldShowResponseStatusBadge('UserQuestionAsked', 'canceled')).toBe(false);
  });

  it('still shows the canceled badge on a regular MessageReceived exchange', () => {
    expect(shouldShowResponseStatusBadge('MessageReceived', 'canceled')).toBe(true);
  });

  it('still shows the canceled badge on a CodingAgentPermissionRequest exchange', () => {
    expect(shouldShowResponseStatusBadge('CodingAgentPermissionRequest', 'canceled')).toBe(true);
  });

  it('shows all non-canceled status badges on a UserQuestionAsked exchange', () => {
    expect(shouldShowResponseStatusBadge('UserQuestionAsked', 'done')).toBe(true);
    expect(shouldShowResponseStatusBadge('UserQuestionAsked', 'working')).toBe(true);
    expect(shouldShowResponseStatusBadge('UserQuestionAsked', 'awaiting')).toBe(true);
    expect(shouldShowResponseStatusBadge('UserQuestionAsked', 'aborted')).toBe(true);
    expect(shouldShowResponseStatusBadge('UserQuestionAsked', 'error')).toBe(true);
  });
});
