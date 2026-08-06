import { describe, it, expect } from 'vitest';
import type { ComponentChildren, VNode } from 'preact';
import { eventWaitIndicatorBody, eventWaitPanelBody } from '../EventWaitPanel';
import type { EventWaitSummary } from '../../../store/thread-events';

/** Flatten a vnode tree into HTML-ish text preserving class / data-* / aria-*
 *  attributes. Same helper as `todo-list-panel.test.tsx` beside it. */
function vnodeToText(node: ComponentChildren): string {
  if (node === null || node === undefined || typeof node === 'boolean') return '';
  if (typeof node === 'string' || typeof node === 'number') return String(node);
  if (Array.isArray(node)) return node.map(vnodeToText).join('');
  const v = node as VNode<Record<string, unknown> & { children?: ComponentChildren }>;
  const tag = typeof v.type === 'string' ? v.type : '';
  const attrs: string[] = [];
  for (const [k, val] of Object.entries(v.props ?? {})) {
    if (k === 'children') continue;
    if (k.startsWith('on')) continue;
    if (val === undefined || val === null || val === false) continue;
    attrs.push(` ${k}="${val}"`);
  }
  const inner = vnodeToText(v.props?.children);
  return tag ? `<${tag}${attrs.join('')}>${inner}</${tag}>` : inner;
}

const NOOP = () => {};

function wait(over: Partial<EventWaitSummary> = {}): EventWaitSummary {
  return {
    wait_id: 'w1',
    reason: 'Waiting for the two remaining coding-agent threads to land',
    on: [{ event_type: 'ChangeProposed' }, { event_type: 'CodingAgentIdled' }],
    expires_at: new Date(Date.now() + 3_600_000).toISOString(),
    ...over,
  } as EventWaitSummary;
}

describe('eventWaitIndicatorBody', () => {
  it('renders nothing when the thread holds no live wait', () => {
    expect(eventWaitIndicatorBody({ waits: [], onClick: NOOP })).toBeNull();
  });

  it('puts the single wait reason in the tooltip, and a count when there are several', () => {
    const one = vnodeToText(eventWaitIndicatorBody({ waits: [wait({ reason: 'until v0.23.0' })], onClick: NOOP }));
    expect(one).toContain('data-tooltip="until v0.23.0"');
    const many = vnodeToText(
      eventWaitIndicatorBody({ waits: [wait(), wait({ wait_id: 'w2' })], onClick: NOOP }),
    );
    expect(many).toContain('data-tooltip="2 subscriptions"');
  });
});

// ──────────────────────────────────────────────────────────────────────────
// Panel: the close button lives in the shell's header strip, never floating
// over the rows. It was absolutely positioned in the panel's top-right corner,
// so it sat on top of the description as soon as that description wrapped (and
// scrolled away with the content of a long list). The structure is what fixes
// that, so the structure is what this pins.
// ──────────────────────────────────────────────────────────────────────────

describe('eventWaitPanelBody', () => {
  it('gives the close button its own header strip, above the list', () => {
    const text = vnodeToText(eventWaitPanelBody({ threadId: 't1', waits: [wait()], onClose: NOOP }));
    const head = text.indexOf('prompt-bar-popover-head');
    const close = text.indexOf('prompt-bar-popover-close');
    const list = text.indexOf('event-wait-panel-list');
    expect(head).toBeGreaterThanOrEqual(0);
    expect(close).toBeGreaterThan(head);
    expect(list).toBeGreaterThan(close);
    expect(text).toContain('aria-label="Close subscriptions"');
    expect(text).toContain('>Subscriptions<');
  });

  it('renders the list inside the padded body, not directly on the shell', () => {
    const text = vnodeToText(eventWaitPanelBody({ threadId: 't1', waits: [wait()], onClose: NOOP }));
    expect(text).toContain('<div class="prompt-bar-popover-body"><ul class="event-wait-panel-list">');
  });

  it('renders one header strip however many waits are live', () => {
    const text = vnodeToText(
      eventWaitPanelBody({
        threadId: 't1',
        waits: [wait({ wait_id: 'a' }), wait({ wait_id: 'b' })],
        onClose: NOOP,
      }),
    );
    // Rows are their own component, so they render as an empty vnode here;
    // the list container plus the head is what this body owns.
    expect(text).toContain('event-wait-panel-list');
    expect((text.match(/prompt-bar-popover-head/g) ?? []).length).toBe(1);
  });
});
