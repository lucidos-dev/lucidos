import { describe, it, expect } from 'vitest';
import type { VNode } from 'preact';
import { describeExecutor } from './ChatExchange';
import { ClaudeIcon } from '../shared/icons';
import { LUCIDOS_AGENT_ICON, LUCIDOS_AGENT_LABEL } from '../../store/thread-events';

describe('describeExecutor', () => {
  it('shows Claude Code label and icon for CC threads', () => {
    const { icon, label } = describeExecutor(true);
    expect(label).toBe('Claude Code');
    expect((icon as VNode).type).toBe(ClaudeIcon);
  });

  it('shows Lucidos Agent label for non-CC threads (same entity as the parent_thread initiator label)', () => {
    expect(describeExecutor(false)).toEqual({
      icon: LUCIDOS_AGENT_ICON,
      label: LUCIDOS_AGENT_LABEL,
    });
  });
});
