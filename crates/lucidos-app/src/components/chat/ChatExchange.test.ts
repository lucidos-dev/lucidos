import { describe, it, expect } from 'vitest';
import type { VNode } from 'preact';
import { describeExecutor, shouldShowResponseStatusBadge } from './ChatExchange';
import { ClaudeIcon, CodexIcon } from '../shared/icons';
import { LUCIDOS_AGENT_ICON, LUCIDOS_AGENT_LABEL } from '../../store/thread-events';

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

  it('shows Lucidos Agent label for non-CC threads (same entity as the parent_thread initiator label)', () => {
    expect(describeExecutor(false)).toEqual({
      icon: LUCIDOS_AGENT_ICON,
      label: LUCIDOS_AGENT_LABEL,
    });
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
