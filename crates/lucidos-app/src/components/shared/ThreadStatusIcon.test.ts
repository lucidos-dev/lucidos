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
    expect(resolveVisualStatus('paused', false, false)).toBe('paused');
  });

  // Outranks `changes` for the same reason `failed` does. In practice the pair
  // never collide: the backend resolves them first, writing `waiting` (not
  // `paused`) for an interrupted thread that already proposed a change.
  it('outranks a proposed change and active children', () => {
    expect(resolveVisualStatus('paused', true, true)).toBe('paused');
  });

  it('paints the neutral paused dot, not progress-dot-failed', () => {
    const vnode = ThreadStatusIcon({ status: 'paused' }) as unknown as {
      props: { children: unknown[]; 'data-tooltip-title': string };
    };
    const classes = (vnode.props.children as ({ props?: { class?: string } } | false)[])
      .filter((c): c is { props: { class: string } } => !!c && typeof c === 'object' && !!c.props?.class)
      .map((c) => c.props.class);
    expect(classes).toContain('progress-dot progress-dot-paused');
    expect(classes.join(' ')).not.toContain('progress-dot-failed');
    expect(vnode.props['data-tooltip-title']).toBe('Paused');
  });
});
