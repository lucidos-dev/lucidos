import { describe, it, expect } from 'vitest';
import { parseToastMessage } from './toastMessage';

describe('parseToastMessage', () => {
  it('returns plain heading and no sections for single-line text', () => {
    const out = parseToastMessage('Hello');
    expect(out).toEqual({ heading: 'Hello', sections: [] });
  });

  it('preserves bullets within a single section under the heading', () => {
    const message = 'Top heading\n• one\n• two';
    const out = parseToastMessage(message);
    expect(out).toEqual({
      heading: 'Top heading',
      sections: [{ title: undefined, bullets: ['one', 'two'] }],
    });
  });

  it('treats blank lines as section separators and the first non-bullet line as the section title', () => {
    const message = [
      'Engine restart required to apply changes:',
      '',
      'Fix toast detail',
      '• feat: add commits column',
      '• fix: backfill restart groups',
      '',
      'Update scheduler',
      '• refactor: extract job runner',
    ].join('\n');

    const out = parseToastMessage(message);
    expect(out.heading).toBe('Engine restart required to apply changes:');
    expect(out.sections).toEqual([
      {
        title: 'Fix toast detail',
        bullets: ['feat: add commits column', 'fix: backfill restart groups'],
      },
      {
        title: 'Update scheduler',
        bullets: ['refactor: extract job runner'],
      },
    ]);
  });

  it('section without commits keeps title and empty bullets array', () => {
    const message = 'Top\n\nThread without commits';
    const out = parseToastMessage(message);
    expect(out.sections).toEqual([{ title: 'Thread without commits', bullets: [] }]);
  });
});
