import { describe, it, expect } from 'vitest';
import type { VNode } from 'preact';
import { InlineStep } from '../chat-exchange-parts';
import { vnodeToText } from './vnodeToText';
import type { ResponseEvent, StepOutcome } from '../../../store/types';

/** How an inline step row renders per outcome. The killed-mid-call row
 *  ('unfinished') is the one that has to be legible at a glance: a struck,
 *  muted row among green checks, so the user can see WHICH step the kill
 *  landed on. It must not shimmer (nothing is running) and must not carry a
 *  checkmark (the tool never reported anything). */
function row(outcome: StepOutcome) {
  const event: Extract<ResponseEvent, { type: 'step' }> = {
    type: 'step',
    description: 'Run shellcheck scripts/lib/webkit_shard.sh',
    tool_name: 'Bash',
    outcome,
  };
  const vnode = InlineStep({ event }) as VNode<{ class?: string; 'data-tooltip'?: string }>;
  return { text: vnodeToText(vnode), props: vnode.props };
}

describe('InlineStep rendering per step outcome', () => {
  it('unfinished: muted struck row, no checkmark, no shimmer', () => {
    const { text, props } = row('unfinished');
    expect(props.class).toContain('unfinished');
    // The distinguishing marker. A ✓ here would be the original lie.
    expect(text).toContain('⊘');
    expect(text).not.toContain('✓');
    // The row is terminal: shimmering it would animate work nobody is doing.
    expect(text).not.toContain('running-shimmer');
    // Named for the user without a trip through the detail modal.
    expect(props['data-tooltip']).toBe('Did not finish');
  });

  it('pending: shimmering description, no icon (the shimmer is the affordance)', () => {
    const { text, props } = row('pending');
    expect(props.class).toContain('pending');
    expect(text).toContain('running-shimmer');
    expect(text).not.toContain('⊘');
    expect(text).not.toContain('✓');
    expect(props['data-tooltip']).toBeUndefined();
  });

  it('success: checkmark, nothing muted or struck', () => {
    const { text, props } = row('success');
    expect(props.class).toContain('success');
    expect(props.class).not.toContain('unfinished');
    expect(text).toContain('✓');
    expect(text).not.toContain('running-shimmer');
  });

  it('error: the failure marker, distinct from unfinished', () => {
    const { text, props } = row('error');
    expect(props.class).toContain('error');
    expect(text).toContain('⚠');
    expect(text).not.toContain('⊘');
  });
});
