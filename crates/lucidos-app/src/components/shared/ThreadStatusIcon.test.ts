import { describe, it, expect } from 'vitest';
import { resolveVisualStatus, statusTooltip, ThreadStatusIcon, type VisualStatus } from './ThreadStatusIcon';

describe('statusTooltip', () => {
  it('titles the tooltip with the status label and explains it in the body', () => {
    expect(statusTooltip('running')).toEqual({
      title: 'Running',
      text: 'Actively working on a response.',
    });
    expect(statusTooltip('question').title).toBe('Waiting for your answer');
    expect(statusTooltip('changes').text).toMatch(/coding agent proposed changes/);
  });

  // One dot now carries both ways a thread can be finished-but-not-done, so its
  // explanation has to name both. Naming only children was true until an event
  // wait could land here, and it would have read as a lie on a thread that has
  // no children at all.
  it('explains BOTH causes of the waiting dot, children and event waits', () => {
    const { title, text } = statusTooltip('waiting');
    expect(title).toBe('Waiting');
    expect(text).toMatch(/child thread/);
    expect(text).toMatch(/subscribed/);
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

// An engine restart interrupted the turn. Nothing failed, and after a *Switch to
// new version* the engine resumes it by itself, so painting the red error dot
// (which also claimed a needs-attention slot) was wrong on both counts.
describe('paused', () => {
  it('resolves to its own visual status, never failed', () => {
    expect(resolveVisualStatus('paused', false, false, false)).toBe('paused');
  });

  // Outranks `changes` for the same reason `failed` does. In practice the pair
  // never collide: the backend resolves them first, writing `waiting` (not
  // `paused`) for an interrupted thread that already proposed a change.
  it('outranks a proposed change and active children', () => {
    expect(resolveVisualStatus('paused', true, true, true)).toBe('paused');
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

// A thread holding a live *event wait* is asleep on purpose. Its backend
// status is plain `idle` (ADR 0049), so before this it fell through to the
// no-dot `idle` branch and rendered exactly like a thread that had finished.
describe('live event waits', () => {
  it('resolves to waiting on their own, with no children and no proposal', () => {
    expect(resolveVisualStatus('idle', false, false, true)).toBe('waiting');
  });

  it('still resolves to idle with no subscription and no children', () => {
    expect(resolveVisualStatus('idle', false, false, false)).toBe('idle');
  });

  // The turn wins while it is running: the thread is not merely watching, it
  // is working, and the subscription indicator says what it is watching for.
  it('does not mask a running turn', () => {
    expect(resolveVisualStatus('running', false, false, true)).toBe('running');
  });

  // A proposed change needs the user; a subscription does not. Hiding the
  // changes dot behind a wait would hide the Apply the user is looking for.
  it('yields to a proposed change', () => {
    expect(resolveVisualStatus('idle', false, true, true)).toBe('changes');
  });

  it('reads the same as active children, which is the point', () => {
    expect(resolveVisualStatus('idle', true, false, false))
      .toBe(resolveVisualStatus('idle', false, false, true));
  });
});
