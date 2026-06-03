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

  it('autoresize is gated on !editing (display:none scrollHeight=0 collapses height to ~2px)', () => {
    // The display textarea has display:none while editing. Calling
    // autoResizeTextarea on a display:none element pins style.height to the
    // border-only height (~2px), and the next render reuses that value once
    // the editor closes — title appears to disappear after a mobile rename.
    // The race fires when SSE delivers ThreadTitleRenamed before the rename
    // HTTP response resolves.
    const effect = source.match(
      /useEffect\(\(\)\s*=>\s*\{[\s\S]*?autoResizeTextarea\(displayRef\.current\)[\s\S]*?\},\s*\[([^\]]*)\]\)/,
    );
    expect(effect, 'autoResizeTextarea must run inside a guarded useEffect').not.toBeNull();
    expect(effect![0]).toMatch(/if\s*\(\s*!\s*editing\s*\)/);
    expect(effect![1]).toContain('editing');
  });

  it('uses .select() for the input', () => {
    expect(source).toMatch(/inputRef\.current\??\.select\(\)/);
  });

  it('observes the display textarea size to re-fit on container width changes', () => {
    // The display textarea wraps based on its parent's width. autoResizeTextarea
    // running only on [title, editing] dep changes leaves style.height pinned
    // to a wrapped-narrow measurement after the container widens (drawer
    // toggle, divider drag, window resize) — the header balloons until rename
    // or reload. A ResizeObserver re-runs the resize on width-only changes.
    expect(source).toMatch(/new ResizeObserver\(/);
    expect(source).toMatch(/observer\.observe\(el\)/);
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
