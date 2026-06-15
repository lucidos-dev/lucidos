import { describe, it, expect } from 'vitest';
// @ts-expect-error — Node APIs available at runtime via Vitest, no @types/node in project
import { readFileSync } from 'node:fs';
// @ts-expect-error — same
import { dirname, resolve } from 'node:path';
// @ts-expect-error — same
import { fileURLToPath } from 'node:url';
import { normalizeRename } from '../ThreadTitleEditor';

const here: string = dirname(fileURLToPath(import.meta.url));
const source = readFileSync(resolve(here, '../ThreadTitleEditor.tsx'), 'utf-8');
// Comment-stripped copy for structural tag counts — prose mentioning
// `<textarea>` / `<input>` in JSX comments must not be counted as markup.
const code: string = source
  .replace(/\/\*[\s\S]*?\*\//g, '')
  .replace(/\/\/[^\n]*/g, '');

describe('ThreadTitleEditor — display vs edit fields', () => {
  it('edits via an <input type="text"> on desktop (single-line hug)', () => {
    expect(source).toMatch(/<input\b[\s\S]*?type=['"]text['"]/);
  });

  it('edits via a <textarea> on mobile (multi-line wrap), gated on viewportIsMobile', () => {
    // A long title must wrap to multiple lines while editing on mobile; an
    // <input> can only ever scroll on one line, so the mobile branch is a
    // <textarea> sized to its content by autoResizeTextarea.
    expect(source).toMatch(/isMobile\s*\?\s*\(\s*<textarea\b/);
    expect(source).toMatch(/viewportIsMobile/);
    expect(source).toMatch(/autoResizeTextarea/);
  });

  it('renders a read-only <div> for display — the only <textarea> is the edit field', () => {
    // The display is a <div>: in the desktop header CSS gives it
    // white-space:nowrap + width:max-content so it hugs the title on one line.
    // A <textarea> sizes to its cols attribute instead and wraps early — it's
    // the mobile EDIT field, never the display.
    expect(code).toMatch(/<div\b[\s\S]*?thread-title-display/);
    const textareas = code.match(/<textarea\b[\s\S]*?>/g) ?? [];
    expect(textareas.length, 'exactly one <textarea> (the mobile edit field)').toBe(1);
    expect(textareas[0]).toContain('thread-title-edit-input');
    expect(textareas[0], 'display must not be a textarea').not.toContain('thread-title-display');
  });

  it('keeps the edit field and the display in the DOM at all times (iOS sync-focus requirement)', () => {
    // The edit field is split by viewport (isMobile ? textarea : input), never
    // gated on `editing` — so a field always exists to focus() synchronously.
    expect(source).not.toMatch(/\{\s*editing\s*&&\s*<input\b/);
    expect(source).not.toMatch(/\{\s*editing\s*&&\s*<textarea\b/);
    expect(source).not.toMatch(/\{\s*!\s*editing\s*&&\s*<div\b[\s\S]*?thread-title-display/);
  });

  it('uses keydown Enter to save', () => {
    expect(source).toMatch(/e\.key === ['"`]Enter['"`]/);
    expect(source).toMatch(/e\.preventDefault\(\)/);
  });

  it('sizes the desktop edit input to its value via the size attribute (hugs the title while editing)', () => {
    expect(source).toMatch(/size=\{[^}]*editValue\.length[^}]*\}/);
  });

  it('uses .select() for the edit field', () => {
    expect(source).toMatch(/inputRef\.current\??\.select\(\)/);
  });
});

describe('normalizeRename', () => {
  // Production case: thread b046ae3e on 2026-05-15. The user clicked the title
  // editor while the title was still the pre-LLM previewText fallback. SSE
  // delivered ThreadTitleGenerated mid-edit; the [title, editing] useEffect
  // skipped the editValue sync to protect typing in progress, leaving
  // editValueRef.current holding the pre-SSE title. A subsequent blur fired
  // save(editValueRef.current) and POSTed the stale value back to /rename,
  // overwriting the LLM title. Tracking "did the user actually type" makes
  // that path a no-op without relying on a stale-snapshot comparison.
  it('returns null when the user did not type (isDirty=false)', () => {
    expect(normalizeRename('anything', 'old', false)).toBe(null);
  });

  it('returns null on empty / whitespace-only input', () => {
    expect(normalizeRename('', 'foo', true)).toBe(null);
    expect(normalizeRename('   ', 'foo', true)).toBe(null);
  });

  it('returns null when trimmed value matches the current title', () => {
    expect(normalizeRename('foo', 'foo', true)).toBe(null);
    expect(normalizeRename('  foo  ', 'foo', true)).toBe(null);
  });

  it('returns the trimmed value when the user typed a genuinely new title', () => {
    expect(normalizeRename('  user typed  ', 'old title', true)).toBe('user typed');
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
