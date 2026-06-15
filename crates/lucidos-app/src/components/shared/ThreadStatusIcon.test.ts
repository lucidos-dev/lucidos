import { describe, it, expect } from 'vitest';
import { statusTooltip, type VisualStatus } from './ThreadStatusIcon';

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
    const hoverable: VisualStatus[] = ['running', 'waiting', 'question', 'changes', 'failed'];
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
