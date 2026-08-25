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
    // The dot now outranks `changes`, so a parked thread with a real change
    // wears it. The copy has to account for the change it is covering.
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

// An engine restart interrupted the turn. Nothing failed, and after a *Switch to
// new version* the engine resumes it by itself, so painting the red error dot
// (which also claimed a needs-attention slot) was wrong on both counts.
describe('paused', () => {
  it('resolves to its own visual status, never failed', () => {
    expect(resolveVisualStatus('paused', false, false, false)).toBe('paused');
  });

  // Outranks `changes` for the same reason `failed` does, and this is the ONLY
  // place that precedence lives. The backend used to resolve it first, writing
  // `waiting` instead of the verdict for an interrupted thread with a change.
  // That lost the verdict to the dying turn's drain, so the pair reaches here
  // now and this case is live rather than theoretical.
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
  // is working, and the waiting indicator says what it is watching for.
  it('does not mask a running turn', () => {
    expect(resolveVisualStatus('running', false, false, true)).toBe('running');
  });

  // A parked thread's change is not final: it wakes on its delivery and may
  // commit again on the same branch. Reading it as "Changes to review" invited
  // an Apply that merges a live branch. The gate in `availableThreadActions`
  // now withholds the button on these same two facts.
  it('outranks a proposed change', () => {
    expect(resolveVisualStatus('idle', false, true, true)).toBe('waiting');
  });

  it('reads the same as active children, which is the point', () => {
    expect(resolveVisualStatus('idle', true, false, false))
      .toBe(resolveVisualStatus('idle', false, false, true));
  });

  // Every combination of the two waiting causes against a proposed change,
  // pinned so the precedence cannot be flipped back by accident.
  it('resolves the four cause combinations', () => {
    expect(resolveVisualStatus('idle', false, false, false)).toBe('idle');
    expect(resolveVisualStatus('idle', false, true, false)).toBe('changes');
    expect(resolveVisualStatus('idle', true, true, false)).toBe('waiting');
    expect(resolveVisualStatus('idle', true, true, true)).toBe('waiting');
  });

  // The verdict statuses stay ahead of both: they describe what happened to the
  // turn, which outranks what the thread is watching for.
  it('never masks failed, running, question or paused', () => {
    expect(resolveVisualStatus('failed', true, true, true)).toBe('failed');
    expect(resolveVisualStatus('running', true, true, true)).toBe('running');
    expect(resolveVisualStatus('waiting_for_user_answer', true, true, true)).toBe('question');
    expect(resolveVisualStatus('paused', true, true, true)).toBe('paused');
  });
});
