import { describe, it, expect } from 'vitest';
import { controlPickToast } from '../controlPickToast';

describe('controlPickToast', () => {
  it('names the pair alone when both halves apply now', () => {
    expect(controlPickToast('Model', 'Opus · High', 'ok', 'ok')).toBe('Model: Opus · High');
    expect(controlPickToast('Model', 'Opus', 'ok', null)).toBe('Model: Opus');
  });

  it('says the effort waits when only the effort does (Claude Code mid-turn)', () => {
    expect(controlPickToast('Model', 'Opus · Low', 'ok', 'next-turn'))
      .toBe('Model: Opus · Low. Effort applies from the next turn');
  });

  it('says the whole pick waits when both halves do (Codex mid-turn)', () => {
    expect(controlPickToast('Model', 'GPT · Low', 'next-turn', 'next-turn'))
      .toBe('Model: GPT · Low. Applies from the next turn');
    expect(controlPickToast('Model', 'GPT', 'next-turn', null))
      .toBe('Model: GPT. Applies from the next turn');
  });
});
