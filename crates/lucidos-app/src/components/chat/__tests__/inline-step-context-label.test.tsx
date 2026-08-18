import { describe, it, expect, afterEach } from 'vitest';
import type { VNode } from 'preact';
import { InlineStep, contextLabel } from '../chat-exchange-parts';
import { vnodeToText } from './vnodeToText';
import { contextViewer } from '../../../store/store';
import type { ContextCapture, ResponseEvent } from '../../../store/types';

/** The context counter on a step row. In a narrow row the full
 *  "178k / 1M (18%)" does not fit beside the description, and the row is
 *  `nowrap`, so the compact form keeps only the percentage. */
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
});

/** Which form is SHOWN is a width question the row answers in CSS (the
 *  `step-row` container query in steps.css), so the component's job is to put
 *  both in the DOM under the classes that gate reads. Rendering one of them
 *  from a viewport signal is the regression this pins: it re-gates on the
 *  device, and a desktop thread pane dragged narrow gets the crowded row back. */
describe('InlineStep context suffix', () => {
  const capture: ContextCapture = {
    producer: 'main_llm',
    model: 'claude-opus-5',
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

/** The counter is the context viewer's only door, now that a thinking pass no
 *  longer gets a row of its own to click. So it has to BE a button wherever
 *  there is a snapshot behind it, and must not pretend to be one where there
 *  isn't. */
describe('InlineStep context counter as a click target', () => {
  const capture: ContextCapture = {
    producer: 'main_llm',
    model: 'claude-opus-5',
    context_window: 1_000_000,
    sections: [],
    tools: [],
    estimated_total_tokens: 178_000,
    trimmed: false,
  };

  function counterOf(event: Extract<ResponseEvent, { type: 'step' }>) {
    const vnode = InlineStep({ event }) as VNode<{ children?: unknown }>;
    const children = vnode.props.children as VNode<Record<string, unknown>>[];
    return children[1];
  }

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
    // The viewer opens from a row and would otherwise be a wall of sections
    // with no statement of which call it belongs to.
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
