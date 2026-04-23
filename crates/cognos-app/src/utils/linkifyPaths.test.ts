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
});
