import { describe, it, expect, afterEach } from 'vitest';
import type { VNode } from 'preact';
import { InlineStep, StepContext, contextLabel } from '../chat-exchange-parts';
import { vnodeToText } from './vnodeToText';
import { contextViewer } from '../../../store/store';
import type { ContextCapture, ResponseEvent } from '../../../store/types';

describe('contextLabel', () => {
  it('full: used, window and percent', () => {
    expect(contextLabel(178_000, 1_000_000, null, false)).toBe('178k / 1M (18%)');
  });

  it('compact: the percentage alone', () => {
    expect(contextLabel(178_000, 1_000_000, null, true)).toBe('18%');
  });

  it('no window: token count, with the message count only in the full form', () => {
    expect(contextLabel(178_000, null, 42, false)).toBe('178k tokens, 42 msgs');
    expect(contextLabel(178_000, null, 42, true)).toBe('178k');
  });

  it('no window and no message count: just the tokens', () => {
    expect(contextLabel(900, null, null, false)).toBe('900 tokens');
  });

  it('over the window: reports the real percentage, never a clamped 100%', () => {
    expect(contextLabel(240_919, 200_000, null, false)).toBe('241k / 200k (120%)');
    expect(contextLabel(240_919, 200_000, null, true)).toBe('120%');
  });
});

describe('InlineStep context suffix', () => {
  const capture: ContextCapture = {
    producer: 'main_llm',
    model: 'claude-opus-5[1m]',
    context_window: 1_000_000,
    sections: [],
    tools: [],
    estimated_total_tokens: 178_000,
    trimmed: false,
  };

  function text(): string {
    const event: Extract<ResponseEvent, { type: 'step' }> = {
      type: 'step',
      description: 'Run grep -rn "fn active_thread_status"',
      tool_name: 'Bash',
      outcome: 'success',
      contextCapture: capture,
    };
    return vnodeToText(InlineStep({ event }) as VNode);
  }

  it('renders both forms, each under the class the width gate keys on', () => {
    const rendered = text();
    expect(rendered).toContain('<span class="step-context-full">178k / 1M (18%)</span>');
    expect(rendered).toContain('<span class="step-context-compact">18%</span>');
  });
});

describe('StepContext as a click target', () => {
  const capture: ContextCapture = {
    producer: 'main_llm',
    model: 'claude-opus-5[1m]',
    context_window: 1_000_000,
    sections: [],
    tools: [],
    estimated_total_tokens: 178_000,
    trimmed: false,
  };

  const counterOf = (event: Extract<ResponseEvent, { type: 'step' }>) =>
    StepContext({ event }) as VNode<Record<string, unknown>>;

  afterEach(() => { contextViewer.value = null; });

  it('opens the context viewer when the step carries a snapshot', () => {
    const event: Extract<ResponseEvent, { type: 'step' }> = {
      type: 'step',
      description: 'Run ls',
      tool_name: 'Bash',
      outcome: 'success',
      contextCapture: capture,
    };
    const counter = counterOf(event);
    expect(counter.type).toBe('button');
    (counter.props.onClick as () => void)();
    expect(contextViewer.value?.snapshot).toBe(capture);
    expect(contextViewer.value?.description).toBe('Run ls');
  });

  it('stays inert text for a legacy row whose tokens have no snapshot behind them', () => {
    const event: Extract<ResponseEvent, { type: 'step' }> = {
      type: 'step',
      description: 'Thinking',
      outcome: 'success',
      context_tokens: 23_500,
    };
    const counter = counterOf(event);
    expect(counter.type).toBe('span');
    expect(counter.props.onClick).toBeUndefined();
  });
});
