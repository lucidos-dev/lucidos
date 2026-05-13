import { describe, it, expect } from 'vitest';
import type { ComponentChildren, VNode } from 'preact';
import { getBannerSlots, DIFF_DISABLED_TOOLTIP } from '../WaitingBanner';

function vnodeText(n: ComponentChildren): string {
  if (n === null || n === undefined || typeof n === 'boolean') return '';
  if (typeof n === 'string' || typeof n === 'number') return String(n);
  if (Array.isArray(n)) return n.map(vnodeText).join('');
  return vnodeText((n as VNode<{ children?: ComponentChildren }>).props.children);
}

function buttonLabels(node: ComponentChildren): string[] {
  if (node === null || node === undefined || typeof node === 'boolean') return [];
  if (typeof node === 'string' || typeof node === 'number') return [];
  if (Array.isArray(node)) return node.flatMap(buttonLabels);
  const v = node as VNode<{ children?: ComponentChildren }>;
  if (v.type === 'button') return [vnodeText(v.props.children).trim()];
  return buttonLabels(v.props.children);
}

function buttonNodes(node: ComponentChildren): VNode<{ disabled?: boolean }>[] {
  if (node === null || node === undefined || typeof node === 'boolean') return [];
  if (typeof node === 'string' || typeof node === 'number') return [];
  if (Array.isArray(node)) return node.flatMap(buttonNodes);
  const v = node as VNode<{ children?: ComponentChildren; disabled?: boolean }>;
  if (v.type === 'button') return [v];
  return buttonNodes(v.props.children);
}

describe('getBannerSlots', () => {
  it('actions state with Diff puts Diff alone in liftable; Save (sectionButtons) and actions in primary', () => {
    const slots = getBannerSlots(
      {
        type: 'actions',
        actions: ['discard', 'apply'],
        threadId: 'tid',
        isArchiving: false,
        requiresRestart: false,
        incomplete: false,
        pendingChange: {
          id: 'change-1',
          request_id: 'req-1',
          thread_id: 'tid',
          thread_title: null,
          branch_name: 'b',
          repo_root: '/x',
          description: 'd',
          file_count: 1,
          files: ['f'],
          requires_restart: false,
          hardened: true,
          status: 'pending',
          created_at: '2026-05-10T00:00:00Z',
          resolved_at: null,
          pre_merge_sha: null,
          post_merge_sha: null,
          commits: [],
          incomplete: false,
        },
        ccDiff: 'enabled',
      },
    );

    expect(buttonLabels(slots.liftable)).toEqual(['Diff']);
    expect(buttonLabels(slots.primary)).toEqual(['Discard', 'Apply']);
  });

  it('actions state on a non-CC thread hides the Diff button', () => {
    const slots = getBannerSlots(
      {
        type: 'actions',
        actions: ['archive'],
        threadId: 'tid',
        isArchiving: false,
        requiresRestart: false,
        incomplete: false,
        pendingChange: null,
        ccDiff: 'hidden',
      },
    );

    expect(slots.liftable).toBeNull();
    expect(buttonLabels(slots.primary)).toEqual(['Archive']);
  });

  it('CC thread with no pending change shows Diff disabled with tooltip', () => {
    const slots = getBannerSlots(
      {
        type: 'actions',
        actions: ['archive'],
        threadId: 'tid',
        isArchiving: false,
        requiresRestart: false,
        incomplete: false,
        pendingChange: null,
        ccDiff: 'disabled',
      },
    );

    expect(buttonLabels(slots.liftable)).toEqual(['Diff']);
    const [diffBtn] = buttonNodes(slots.liftable);
    expect(diffBtn.props.disabled).toBe(true);
    expect((diffBtn.props as { 'data-tooltip'?: string })['data-tooltip']).toBe(DIFF_DISABLED_TOOLTIP);
  });

  it('CC thread with ccDiff=enabled and no pending change uses the thread-level diff click', () => {
    const slots = getBannerSlots(
      {
        type: 'actions',
        actions: ['discard', 'apply'],
        threadId: 'tid',
        isArchiving: false,
        requiresRestart: false,
        incomplete: false,
        pendingChange: null,
        ccDiff: 'enabled',
      },
    );

    expect(buttonLabels(slots.liftable)).toEqual(['Diff']);
    const [diffBtn] = buttonNodes(slots.liftable);
    expect(diffBtn.props.disabled).toBe(false);
    expect(typeof (diffBtn.props as { onClick?: unknown }).onClick).toBe('function');
  });

  it('applying state puts Apply... in primary; nothing liftable', () => {
    const slots = getBannerSlots({ type: 'applying' });
    expect(slots.liftable).toBeNull();
    expect(buttonLabels(slots.primary)).toEqual(['Apply...']);
  });

  it('discarding state puts Discard... in primary; nothing liftable', () => {
    const slots = getBannerSlots({ type: 'discarding' });
    expect(slots.liftable).toBeNull();
    expect(buttonLabels(slots.primary)).toEqual(['Discard...']);
  });
});
