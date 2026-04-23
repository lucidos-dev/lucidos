import { describe, it, expect } from 'vitest';
import type { VNode } from 'preact';
import { describeExecutor } from './ChatExchange';
import { ClaudeIcon } from '../shared/icons';

describe('describeExecutor', () => {
  it('shows Claude Code label and icon for CC threads', () => {
    const { icon, label } = describeExecutor(true);
    expect(label).toBe('Claude Code');
    expect((icon as VNode).type).toBe(ClaudeIcon);
  });

  it('shows Lucidos label for non-CC threads (triggers and chat both invoke Lucidos)', () => {
    expect(describeExecutor(false)).toEqual({
      icon: '💡',
      label: 'Lucidos',
    });
  });
});
