import { describe, it, expect, afterEach } from 'vitest';
import type { VNode } from 'preact';
import { InlineStep, contextLabel } from '../chat-exchange-parts';
import { vnodeToText } from './vnodeToText';
import { viewportIsMobile } from '../../../utils/viewport';
import type { ContextCapture, ResponseEvent } from '../../../store/types';

/** The context counter on a step row. On a phone the full
 *  "178k / 1000k (18%)" does not fit beside the description, and the row is
 *  `nowrap`, so mobile keeps only the percentage. */
describe('contextLabel', () => {
  it('desktop: used, window and percent', () => {
    expect(contextLabel(178_000, 1_000_000, null, false)).toBe('178k / 1000k (18%)');
  });

  it('mobile: the percentage alone', () => {
    expect(contextLabel(178_000, 1_000_000, null, true)).toBe('18%');
  });

  it('no window: token count, with the message count only on desktop', () => {
    expect(contextLabel(178_000, null, 42, false)).toBe('178k tokens, 42 msgs');
    expect(contextLabel(178_000, null, 42, true)).toBe('178k');
  });

  it('no window and no message count: just the tokens', () => {
    expect(contextLabel(900, null, null, false)).toBe('900 tokens');
  });
});

describe('InlineStep context suffix', () => {
  const wasMobile = viewportIsMobile.peek();
  afterEach(() => { viewportIsMobile.value = wasMobile; });

  const capture: ContextCapture = {
    producer: 'main_llm',
    model: 'claude-opus-5',
    context_window: 1_000_000,
    sections: [],
    tools: [],
    estimated_total_tokens: 178_000,
    trimmed: false,
  };

  function text(mobile: boolean): string {
    viewportIsMobile.value = mobile;
    const event: Extract<ResponseEvent, { type: 'step' }> = {
      type: 'step',
      description: 'Run grep -rn "fn active_thread_status"',
      tool_name: 'Bash',
      outcome: 'success',
      contextCapture: capture,
    };
    return vnodeToText(InlineStep({ event }) as VNode);
  }

  it('renders the full counter on desktop', () => {
    expect(text(false)).toContain('178k / 1000k (18%)');
  });

  it('renders only the percent on mobile', () => {
    const rendered = text(true);
    expect(rendered).toContain('18%');
    expect(rendered).not.toContain('1000k');
  });
});
