import { EventWaitClockIcon } from './icons';
import { describe, it, expect } from 'vitest';
import { statusTooltip, ThreadStatusIcon } from './ThreadStatusIcon';
import { resolveVisualStatus, visualStatusFor, type VisualStatus } from './threadVisualStatus';
import type { ThreadMeta } from '../../store/thread-events';

describe('visualStatusFor', () => {
  const idle = {
    activeChildrenCount: 0,
    awaitedChildrenCount: 0,
    codingAgentChangeState: { kind: 'none' },
    liveEventWaitCount: 0,
  } as ThreadMeta;

  it('reads a parent with an awaited child as waiting, with no child running', () => {
    expect(visualStatusFor('idle', { ...idle, awaitedChildrenCount: 1 })).toBe('waiting');
  });

  it('reads a running child the parent does not await as idle', () => {
    const runningNoWakeChild: ThreadMeta = { ...idle, activeChildrenCount: 1 };
    expect(visualStatusFor('idle', runningNoWakeChild)).toBe('idle');
  });

  it('lets the parent\'s own change outrank an awaited child', () => {
    expect(visualStatusFor('idle', { ...idle, awaitedChildrenCount: 1, codingAgentChangeState: { kind: 'proposed', requires_restart: false }}))
      .toBe('changes');
  });

  it('draws no changes dot for unproposed work', () => {
    const withheld = { kind: 'unproposed', reason: 'turn_incomplete' } as const;
    expect(visualStatusFor('idle', { ...idle, codingAgentChangeState: withheld }))
      .toBe('idle');
    expect(visualStatusFor('idle', { ...idle, awaitedChildrenCount: 1, codingAgentChangeState: withheld }))
      .toBe('waiting');
  });

  it('reads an absent count as zero', () => {
    const beforeTheField = {
      activeChildrenCount: 0,
      codingAgentChangeState: { kind: 'none' },
      liveEventWaitCount: 0,
    } as ThreadMeta;
    expect(visualStatusFor('idle', beforeTheField)).toBe('idle');
  });

  it('resolves on the status alone for a thread it cannot see', () => {
    expect(visualStatusFor('running', undefined)).toBe('running');
    expect(visualStatusFor('idle', undefined)).toBe('idle');
  });
});

describe('statusTooltip', () => {
  it('titles the tooltip with the status label and explains it in the body', () => {
    expect(statusTooltip('running')).toEqual({
      title: 'Running',
      text: 'Actively working on a response.',
    });
    expect(statusTooltip('question').title).toBe('Waiting for your answer');
    expect(statusTooltip('changes').text).toMatch(/coding agent proposed changes/);
  });

  it('explains BOTH causes of the waiting dot, children and event waits', () => {
    const { title, text } = statusTooltip('waiting');
    expect(title).toBe('Waiting');
    expect(text).toMatch(/child thread/);
    expect(text).toMatch(/subscribed/);
    expect(text).toMatch(/proposed change/);
  });

  it('gives every hoverable dot a non-empty title and explanation', () => {
    const hoverable: VisualStatus[] = ['running', 'waiting', 'question', 'changes', 'paused', 'failed'];
    for (const status of hoverable) {
      const { title, text } = statusTooltip(status);
      expect(title.length).toBeGreaterThan(0);
      expect(text.length).toBeGreaterThan(0);
    }
  });

  it('returns empty strings for idle (no dot, so no tooltip)', () => {
    expect(statusTooltip('idle')).toEqual({ title: '', text: '' });
  });
});

describe('paused', () => {
  it('resolves to its own visual status, never failed', () => {
    expect(resolveVisualStatus('paused', false, false, false)).toBe('paused');
  });

  it('outranks a proposed change and active children', () => {
    expect(resolveVisualStatus('paused', true, true, true)).toBe('paused');
    expect(resolveVisualStatus('paused', false, true, false)).toBe('paused');
    expect(resolveVisualStatus('failed', false, true, false)).toBe('failed');
  });

  it('paints the pause glyph, not a dot and never progress-dot-failed', () => {
    const vnode = ThreadStatusIcon({ status: 'paused' }) as unknown as {
      props: { children: unknown[]; 'data-tooltip-title': string };
    };
    const classes = (vnode.props.children as ({ props?: { class?: string } } | false)[])
      .filter((c): c is { props: { class: string } } => !!c && typeof c === 'object' && !!c.props?.class)
      .map((c) => c.props.class);
    expect(classes).toContain('thread-status-paused-icon');
    expect(classes.join(' ')).not.toContain('progress-dot');
    expect(vnode.props['data-tooltip-title']).toBe('Paused');
  });
});

describe('live event waits', () => {
  it('resolves to waiting on their own, with no children and no proposal', () => {
    expect(resolveVisualStatus('idle', false, false, true)).toBe('waiting');
  });

  it('still resolves to idle with no subscription and no children', () => {
    expect(resolveVisualStatus('idle', false, false, false)).toBe('idle');
  });

  it('does not mask a running turn', () => {
    expect(resolveVisualStatus('running', false, false, true)).toBe('running');
  });

  it('outranks a proposed change', () => {
    expect(resolveVisualStatus('idle', false, true, true)).toBe('waiting');
  });

  it('reads the same as active children, which is the point', () => {
    expect(resolveVisualStatus('idle', true, false, false))
      .toBe(resolveVisualStatus('idle', false, false, true));
  });

  it('resolves every cause combination the way the Apply gate does', () => {
    const cases: [boolean, boolean, boolean, VisualStatus][] = [
      [false, false, false, 'idle'],
      [false, true, false, 'changes'],
      [true, false, false, 'waiting'],
      [true, true, false, 'changes'],
      [false, false, true, 'waiting'],
      [false, true, true, 'waiting'],
      [true, false, true, 'waiting'],
      [true, true, true, 'waiting'],
    ];
    for (const [children, proposed, waits, dot] of cases) {
      expect(resolveVisualStatus('idle', children, proposed, waits), `children=${children} proposed=${proposed} waits=${waits}`).toBe(dot);
    }
  });

  it('paints changes, not waiting, on a parent with a change and a running child', () => {
    expect(resolveVisualStatus('idle', true, true, false)).toBe('changes');
  });

  it('never masks failed, running, question or paused', () => {
    expect(resolveVisualStatus('failed', true, true, true)).toBe('failed');
    expect(resolveVisualStatus('running', true, true, true)).toBe('running');
    expect(resolveVisualStatus('waiting_for_user_answer', true, true, true)).toBe('question');
    expect(resolveVisualStatus('paused', true, true, true)).toBe('paused');
  });
});

describe('read request and change marks', () => {
  function classesOf(status: 'read-request' | 'changes'): string[] {
    const vnode = ThreadStatusIcon({ status }) as unknown as { props: { children: unknown[] } };
    return (vnode.props.children as ({ props?: { class?: string } } | false)[])
      .filter((c): c is { props: { class: string } } => !!c && typeof c === 'object' && !!c.props?.class)
      .map((c) => c.props.class);
  }

  it('ranks a read request below a change and above either way of waiting', () => {
    expect(resolveVisualStatus('idle', false, false, false, true)).toBe('read-request');
    expect(resolveVisualStatus('idle', false, true, false, true)).toBe('changes');
    expect(resolveVisualStatus('idle', true, false, false, true)).toBe('read-request');
    expect(resolveVisualStatus('idle', false, false, true, true)).toBe('read-request');
    expect(resolveVisualStatus('idle', false, true, true, false)).toBe('waiting');
    expect(resolveVisualStatus('running', false, false, false, true)).toBe('running');
  });

  it('draws the filled dot for a read request and the diff glyph for a change', () => {
    expect(classesOf('read-request').join(' ')).toContain('progress-dot-read-request');
    expect(classesOf('changes')).toEqual(['thread-status-changes-icon']);
  });
});


describe('waiting clock', () => {
  it('uses the shared static clock for either waiting cause', () => {
    for (const [children, events] of [[true, false], [false, true]]) {
      const status = resolveVisualStatus('idle', children, false, events);
      const vnode = ThreadStatusIcon({ status }) as unknown as {
        props: { children: ({ props?: { class?: string; children?: { type: unknown } } } | false)[] };
      };
      const marks = vnode.props.children.filter(c => !!c && c.props?.class);
      expect(marks).toHaveLength(1);
      const mark = marks[0] as { props: { class: string; children: { type: unknown } } };
      expect(mark.props.class).toBe('thread-status-waiting-icon');
      expect(mark.props.children.type).toBe(EventWaitClockIcon);
    }
  });
});
