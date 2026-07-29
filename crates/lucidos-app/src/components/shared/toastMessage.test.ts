import { describe, it, expect } from 'vitest';
import { composeToastMessage, parseToastMessage } from './toastMessage';

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

describe('composeToastMessage', () => {
  it('keeps a single-line body inline after the title', () => {
    expect(composeToastMessage('Permission needed', 'Edit /tmp/settings.json')).toBe(
      'Permission needed: Edit /tmp/settings.json',
    );
  });

  it('returns the title alone when there is no body', () => {
    expect(composeToastMessage('Backup complete', '')).toBe('Backup complete');
  });

  // A one-item list has no newline, so a newline-only structure test would
  // inline it as "1 change ready to apply: • Set Opus 5 as Default" and the
  // parser would fold the bullet into the heading.
  it('treats a lone bullet as structured despite having no newline', () => {
    const out = parseToastMessage(
      composeToastMessage('1 change ready to apply', '• Set Opus 5 as Default'),
    );
    expect(out.heading).toBe('1 change ready to apply');
    expect(out.sections).toEqual([{ title: undefined, bullets: ['Set Opus 5 as Default'] }]);
  });

  it('gives the title its own heading line when the body carries bullets', () => {
    expect(composeToastMessage('2 changes ready to apply', '• Alpha\n• Beta')).toBe(
      '2 changes ready to apply\n• Alpha\n• Beta',
    );
  });

  // The regression: gluing a multi-line body onto the title with ": " absorbed
  // the body's first line into the heading. With a body whose lead line
  // restated the title that rendered the doubled
  // "1 change ready to apply: 1 change ready to apply"; with a plain bulleted
  // body it silently ate the first bullet.
  it('never absorbs the first body line into the heading', () => {
    const out = parseToastMessage(composeToastMessage('2 changes ready to apply', '• Alpha\n• Beta'));
    expect(out.heading).toBe('2 changes ready to apply');
    expect(out.sections).toEqual([{ title: undefined, bullets: ['Alpha', 'Beta'] }]);
  });
});
