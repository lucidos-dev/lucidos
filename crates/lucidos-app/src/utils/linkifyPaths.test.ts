import { describe, it, expect } from 'vitest';
import { linkifyPaths } from './linkifyPaths';

describe('linkifyPaths', () => {
  it('linkifies bare URLs in text', () => {
    const html = '<p>Visit https://example.com for details</p>';
    const result = linkifyPaths(html, [], []);
    expect(result).toContain('<a href="https://example.com" target="_blank" rel="noopener">');
  });

  it('does not create nested <a> tags for already-linked URLs', () => {
    const html = '<p><a href="https://example.com">https://example.com</a></p>';
    const result = linkifyPaths(html, [], []);
    // Should NOT wrap the link text in another <a>
    expect(result).toBe(html);
  });

  it('does not linkify URLs inside <code> blocks', () => {
    const html = '<p>Use <code>https://localhost:5174/oauth/callback</code></p>';
    const result = linkifyPaths(html, [], []);
    // URL inside <code> should remain as plain text
    expect(result).not.toContain('<a href=');
    expect(result).toContain('<code>https://localhost:5174/oauth/callback</code>');
  });

  it('linkifies artifact paths in text', () => {
    const html = '<p>See user_profile.md for details</p>';
    const result = linkifyPaths(html, ['user_profile.md'], []);
    expect(result).toContain('<a class="artifact-link" data-path="user_profile.md">user_profile.md</a>');
  });

  it('does not linkify artifact paths inside <a> tags', () => {
    const html = '<p><a href="/files">user_profile.md</a></p>';
    const result = linkifyPaths(html, ['user_profile.md'], []);
    expect(result).toBe(html);
  });

  it('linkifies artifact paths inside <code> tags (LLMs wrap paths in backticks)', () => {
    const html = '<p>Run <code>cat user_profile.md</code></p>';
    const result = linkifyPaths(html, ['user_profile.md'], []);
    expect(result).toContain('artifact-link');
    expect(result).toContain('data-path="user_profile.md"');
  });

  it('linkifies app names in text', () => {
    const html = '<p>Use the Todo app</p>';
    const result = linkifyPaths(html, [], [{ name: 'Todo', id: 'todo' }]);
    expect(result).toContain('<a class="app-link" data-app-id="todo">Todo</a>');
  });

  it('does not linkify app names inside <a> tags', () => {
    const html = '<p><a href="/apps">Todo</a></p>';
    const result = linkifyPaths(html, [], [{ name: 'Todo', id: 'todo' }]);
    expect(result).toBe(html);
  });

  it('linkifies app IDs when different from app name', () => {
    const html = '<p>Åpne FINN Jobber: finn-jobs</p>';
    const result = linkifyPaths(html, [], [{ name: 'FINN Jobber', id: 'finn-jobs' }]);
    expect(result).toContain('<a class="app-link" data-app-id="finn-jobs">finn-jobs</a>');
    expect(result).toContain('<a class="app-link" data-app-id="finn-jobs">FINN Jobber</a>');
  });

  it('does not duplicate app entries when name equals id', () => {
    const html = '<p>Use todo for tasks</p>';
    const result = linkifyPaths(html, [], [{ name: 'todo', id: 'todo' }]);
    // Should only linkify once, not create double matches
    expect(result).toContain('<a class="app-link" data-app-id="todo">todo</a>');
  });

  it('handles real-world pulldown_cmark HTML with URLs in <a> and <code>', () => {
    // Actual HTML from the bug report — pulldown_cmark output with auto-linked URL
    // and URL inside <code>
    const html = [
      '<p><strong><a href="https://portal.azure.com/#blade/Microsoft_AAD_RegisteredApps" target="_blank" rel="noopener">',
      'https://portal.azure.com/#blade/Microsoft_AAD_RegisteredApps</a></strong></p>',
      '<ul><li>Redirect URI: <code>https://localhost:5174/oauth/callback</code></li></ul>',
    ].join('');
    const result = linkifyPaths(html, [], []);
    // No nested <a> for the portal URL
    expect(result).not.toMatch(/<a[^>]*><a/);
    // No <a> inside <code>
    expect(result).toContain('<code>https://localhost:5174/oauth/callback</code>');
  });

  it('preserves artifacts/ prefix in data-path for API compatibility', () => {
    const html = '<p>See artifacts/projects/emil/notes.md for details</p>';
    const result = linkifyPaths(html, ['artifacts/projects/emil/notes.md'], []);
    // data-path must keep the artifacts/ prefix so the backend API validation passes
    expect(result).toContain('data-path="artifacts/projects/emil/notes.md"');
    expect(result).toContain('>artifacts/projects/emil/notes.md</a>');
  });

  it('resolves bare path to full store path with artifacts/ prefix', () => {
    const html = '<p>Check projects/emil/notes.md for updates</p>';
    const result = linkifyPaths(html, ['artifacts/projects/emil/notes.md'], []);
    // Even though text omits the prefix, data-path must include it for the API
    expect(result).toContain('data-path="artifacts/projects/emil/notes.md"');
    // Display text should match what the user wrote (without prefix)
    expect(result).toContain('>projects/emil/notes.md</a>');
  });

  it('preserves non-artifacts prefixes as-is (knowhow/, apps/)', () => {
    const html = '<p>Read knowhow/cooking.md</p>';
    const result = linkifyPaths(html, ['knowhow/cooking.md'], []);
    expect(result).toContain('data-path="knowhow/cooking.md"');
  });

  it('handles empty input', () => {
    expect(linkifyPaths('', [], [])).toBe('');
  });

  it('preserves HTML structure with no paths or apps', () => {
    const html = '<p>Hello <strong>world</strong></p>';
    expect(linkifyPaths(html, [], [])).toBe(html);
  });

  it('linkifies paths correctly when path list is very large', () => {
    // Simulates a workspace with thousands of artifacts — personal workspace had 7458
    // when WebKit's YARR threw "regular expression too large" at runtime.
    const paths = Array.from(
      { length: 5000 },
      (_, i) => `artifacts/path/file_${i.toString().padStart(6, '0')}.md`,
    );
    const html = '<p>See artifacts/path/file_002500.md for details</p>';
    const result = linkifyPaths(html, paths, []);
    expect(result).toContain('data-path="artifacts/path/file_002500.md"');
    expect(result).toContain('>artifacts/path/file_002500.md</a>');
  });

  it('prefers longest match across batches (length-desc tiebreak)', () => {
    // With batched regexes, a short prefix and a longer path could land in different
    // batches. The combined match selection must still prefer the longer one.
    const shortPath = 'notes.md';
    // 999 filler paths to push the longer path into a later batch (batch size = 500)
    const filler = Array.from({ length: 999 }, (_, i) => `filler/file_${i}.md`);
    const longPath = 'projects/emil/notes.md';
    const html = '<p>See projects/emil/notes.md</p>';
    const result = linkifyPaths(html, [shortPath, ...filler, longPath], []);
    expect(result).toContain('data-path="projects/emil/notes.md"');
    expect(result).toContain('>projects/emil/notes.md</a>');
    // Must NOT have a nested link of the short path inside the long-path anchor
    expect(result).not.toMatch(/<a[^>]*><a/);
  });

  it('keeps each compiled regex small enough for WebKit ("regex too large" guard)', () => {
    // WebKit's YARR engine throws SyntaxError "regular expression too large" when the
    // source exceeds an internal limit. V8 has no such limit, so this test asserts the
    // structural property directly: no single RegExp constructed by linkifyPaths may have
    // a source approaching the WebKit limit.
    const MAX_SAFE_SOURCE = 100_000;
    const sources: number[] = [];
    const RealRegExp = globalThis.RegExp;
    const Spy: any = function (pattern: any, flags?: string) {
      if (typeof pattern === 'string') sources.push(pattern.length);
      return new RealRegExp(pattern, flags);
    };
    Spy.prototype = RealRegExp.prototype;
    (globalThis as any).RegExp = Spy;
    try {
      const paths = Array.from(
        { length: 10000 },
        (_, i) => `artifacts/path/segment-${i.toString(36)}/file.md`,
      );
      linkifyPaths('<p>hello world</p>', paths, []);
    } finally {
      (globalThis as any).RegExp = RealRegExp;
    }
    expect(sources.length).toBeGreaterThan(0);
    const maxSource = Math.max(...sources);
    expect(maxSource).toBeLessThan(MAX_SAFE_SOURCE);
  });
});
