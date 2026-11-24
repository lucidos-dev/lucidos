import { describe, it, expect } from 'vitest';
import type { VNode } from 'preact';
import { InlineStep } from '../chat-exchange-parts';
import { vnodeToText } from './vnodeToText';
import type { ResponseEvent, StepOutcome } from '../../../store/types';

const MARK: Record<Exclude<StepOutcome, 'pending'>, string> = {
  success: 'step-outcome-icon-success',
  error: 'step-outcome-icon-error',
  unfinished: 'step-outcome-icon-unfinished',
  blocked: 'step-outcome-icon-blocked',
  denied: 'step-outcome-icon-denied',
};

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
    expect(text).toContain(MARK.unfinished);
    expect(text).not.toContain(MARK.success);
    expect(text).not.toContain('running-shimmer');
    expect(props['data-tooltip']).toBe('Did not finish');
  });

  it('pending: shimmering description, no icon (the shimmer is the affordance)', () => {
    const { text, props } = row('pending');
    expect(props.class).toContain('pending');
    expect(text).toContain('running-shimmer');
    expect(text).not.toContain(MARK.unfinished);
    expect(text).not.toContain(MARK.success);
    expect(text).not.toContain('<svg');
    expect(props['data-tooltip']).toBeUndefined();
  });

  it('success: checkmark, nothing muted or struck', () => {
    const { text, props } = row('success');
    expect(props.class).toContain('success');
    expect(props.class).not.toContain('unfinished');
    expect(text).toContain(MARK.success);
    expect(text).not.toContain('running-shimmer');
  });

  it('error: the failure marker, distinct from unfinished', () => {
    const { text, props } = row('error');
    expect(props.class).toContain('error');
    expect(text).toContain(MARK.error);
    expect(text).not.toContain(MARK.unfinished);
    expect(text).not.toContain(MARK.success);
    expect(props['data-tooltip']).toBe('Failed');
  });

  it('blocked: a pause mark, no shimmer, no checkmark', () => {
    const { text, props } = row('blocked');
    expect(props.class).toContain('blocked');
    expect(text).toContain(MARK.blocked);
    expect(text).not.toContain('running-shimmer');
    expect(text).not.toContain(MARK.success);
    expect(props['data-tooltip']).toBe('Needs approval');
  });

  it('denied: the cross, never the success check', () => {
    const { text, props } = row('denied');
    expect(props.class).toContain('denied');
    expect(text).toContain(MARK.denied);
    expect(text).not.toContain(MARK.success);
    expect(text).not.toContain('running-shimmer');
    expect(text).not.toContain(MARK.unfinished);
    expect(props['data-tooltip']).toBe('Denied');
  });
});

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
