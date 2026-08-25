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

  // The reported bug: a command waiting on a permission card rendered exactly
  // like one that was running. Nothing is running, so nothing may shimmer.
  it('blocked: a pause mark, no shimmer, no checkmark', () => {
    const { text, props } = row('blocked');
    expect(props.class).toContain('blocked');
    expect(text).toContain('‖');
    expect(text).not.toContain('running-shimmer');
    expect(text).not.toContain('✓');
    expect(props['data-tooltip']).toBe('Needs approval');
  });

  // The other half: a refused command used to carry the same green check as one
  // that ran and succeeded.
  it('denied: the cross, never the success check', () => {
    const { text, props } = row('denied');
    expect(props.class).toContain('denied');
    expect(text).toContain('✗');
    expect(text).not.toContain('✓');
    expect(text).not.toContain('running-shimmer');
    // Distinct from the killed-mid-call row, which is struck instead.
    expect(text).not.toContain('⊘');
    expect(props['data-tooltip']).toBe('Denied');
  });
});

/** Thinking and the call it produced share one row, so the reasoning ticker has
 *  to know when to stop: it is the only thing an unnamed row can say, and it is
 *  noise once the row can name the tool it called. The full text is in the step
 *  detail either way. */
describe('InlineStep reasoning ticker', () => {
  const reasoning = 'Weighing the options\nChecking the projection first';

  function tickerOf(description: string): string {
    const event: Extract<ResponseEvent, { type: 'step' }> = {
      type: 'step',
      description,
      outcome: 'pending',
      thinkingText: reasoning,
    };
    return vnodeToText(InlineStep({ event }) as VNode);
  }

  it('shows the latest reasoning line while the row is still unnamed', () => {
    expect(tickerOf('Thinking')).toContain('Checking the projection first');
  });

  it('drops it once the row names the call it produced', () => {
    const rendered = tickerOf('Running: cd ~/projects && ls');
    expect(rendered).toContain('Running: cd ~/projects && ls');
    expect(rendered).not.toContain('Checking the projection first');
  });
});
