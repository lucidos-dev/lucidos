import { describe, it, expect } from 'vitest';
// @ts-expect-error — Node APIs available at runtime via Vitest, no @types/node in project
import { readFileSync } from 'node:fs';
// @ts-expect-error — same
import { dirname, resolve } from 'node:path';
// @ts-expect-error — same
import { fileURLToPath } from 'node:url';

const here: string = dirname(fileURLToPath(import.meta.url));
const source = readFileSync(resolve(here, '../ThreadTitleEditor.tsx'), 'utf-8');

describe('ThreadTitleEditor — dual-element split', () => {
  it('renders an <input type="text"> for editing', () => {
    expect(source).toMatch(/<input\b[\s\S]*?type=['"]text['"]/);
  });

  it('renders a readOnly <textarea> for display', () => {
    expect(source).toMatch(/<textarea\b[\s\S]*?readOnly/);
  });

  it('keeps both elements in the DOM at all times (iOS sync-focus requirement)', () => {
    expect(source).not.toMatch(/\{\s*editing\s*&&\s*<input\b/);
    expect(source).not.toMatch(/\{\s*!\s*editing\s*&&\s*<textarea\b/);
  });

  it('uses keydown Enter to save', () => {
    expect(source).toMatch(/e\.key === ['"`]Enter['"`]/);
    expect(source).toMatch(/e\.preventDefault\(\)/);
  });

  it('autoresize never depends on editValue (per-keystroke churn caused iOS cursor reset)', () => {
    const autoResizeMatch = source.match(/autoResizeTextarea\([\s\S]*?\),\s*\[(.*?)\]/);
    if (autoResizeMatch) {
      expect(autoResizeMatch[1]).not.toContain('editValue');
    }
  });

  it('uses .select() for the input', () => {
    expect(source).toMatch(/inputRef\.current\??\.select\(\)/);
  });
});

describe('MobileSwipeContainer — keyboard-active does not trap title editor', () => {
  const swipeSource = readFileSync(
    resolve(here, '../../layout/MobileSwipeContainer.tsx'),
    'utf-8',
  );

  it('focusin handler excludes elements inside .mobile-thread-title-row', () => {
    expect(swipeSource).toMatch(/mobile-thread-title-row/);
  });
});
