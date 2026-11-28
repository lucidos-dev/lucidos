import { describe, it, expect } from 'vitest';
import { answerIsQuiet } from './quietAnswer';
import type { QuestionOption } from './thread-events';

const options: QuestionOption[] = [
  { id: 'loud', label: 'Apply now' },
  { id: 'quiet-a', label: 'Not now', quiet: true },
  { id: 'quiet-b', label: 'Skip', quiet: true },
];

describe('answerIsQuiet', () => {
  it('follows the engine rule: only quiet picks, and typed words always wake', () => {
    expect(answerIsQuiet({ kind: 'Selected', option_id: 'quiet-a' }, options)).toBe(true);
    expect(answerIsQuiet({ kind: 'Selected', option_id: 'loud' }, options)).toBe(false);
    expect(answerIsQuiet({ kind: 'MultiSelected', option_ids: ['quiet-a', 'quiet-b'] }, options)).toBe(true);
    expect(answerIsQuiet({ kind: 'MultiSelected', option_ids: ['quiet-a', 'loud'] }, options)).toBe(false);
    expect(answerIsQuiet({ kind: 'MultiSelected', option_ids: [] }, options)).toBe(false);
    expect(answerIsQuiet({ kind: 'MultiSelected', option_ids: ['quiet-a'], text: ' but why' }, options)).toBe(false);
    expect(answerIsQuiet({ kind: 'MultiSelected', option_ids: ['quiet-a'], text: '  ' }, options)).toBe(true);
    expect(answerIsQuiet({ kind: 'MultiSelected', option_ids: ['quiet-a'], image_hashes: ['h'] }, options)).toBe(false);
    expect(answerIsQuiet({ kind: 'FreeText', text: 'Not now' }, options)).toBe(false);
  });
});
