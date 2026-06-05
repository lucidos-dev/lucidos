import { describe, it, expect } from 'vitest';
import { renderMarkdown, renderMarkdownInline, renderMarkdownInlineWithLinks } from './renderMarkdown';

describe('renderMarkdown', () => {
  it('converts basic markdown to HTML', () => {
    const html = renderMarkdown('**bold** and *italic*');
    expect(html).toContain('<strong>bold</strong>');
    expect(html).toContain('<em>italic</em>');
  });

  it('converts headings', () => {
    const html = renderMarkdown('# Heading 1\n## Heading 2');
    expect(html).toContain('<h1');
    expect(html).toContain('Heading 1');
    expect(html).toContain('<h2');
    expect(html).toContain('Heading 2');
  });

  it('converts soft breaks to hard breaks', () => {
    const html = renderMarkdown('line one\nline two');
    expect(html).toContain('<br');
  });

  it('converts lists', () => {
    const html = renderMarkdown('- item one\n- item two\n- item three');
    expect(html).toContain('<ul>');
    expect(html).toContain('<li>');
    expect(html).toContain('item one');
    expect(html).toContain('item two');
  });

  it('handles code blocks with copy button', () => {
    const html = renderMarkdown('```js\nconsole.log("hi")\n```');
    expect(html).toContain('<div class="code-block-wrapper">');
    expect(html).toContain('code-block-copy-btn');
    expect(html).toContain('<code>');
    expect(html).toContain('console.log');
  });

  it('shows language label on code blocks', () => {
    const html = renderMarkdown('```python\nprint("hi")\n```');
    expect(html).toContain('<span class="code-block-lang">python</span>');
  });

  it('escapes HTML in language label to prevent XSS', () => {
    const html = renderMarkdown('```js<script>alert(1)</script>\ncode\n```');
    expect(html).not.toContain('<script>');
    expect(html).toContain('&lt;script&gt;');
  });

  it('renders code blocks without language', () => {
    const html = renderMarkdown('```\nsome code\n```');
    expect(html).toContain('code-block-wrapper');
    expect(html).toContain('code-block-copy-btn');
    expect(html).not.toContain('code-block-lang');
  });

  it('handles inline code', () => {
    const html = renderMarkdown('use `foo()` here');
    expect(html).toContain('<code>foo()</code>');
  });

  it('handles tables', () => {
    const md = '| Name | Value |\n| --- | --- |\n| A | 1 |';
    const html = renderMarkdown(md);
    expect(html).toContain('<div class="table-scroll-wrapper"><table>');
    expect(html).toContain('</table></div>');
    expect(html).toContain('<th>');
    expect(html).toContain('Name');
  });

  it('handles empty input', () => {
    expect(renderMarkdown('')).toBe('');
  });

  it('handles plain text without markdown', () => {
    const html = renderMarkdown('just plain text');
    expect(html).toContain('just plain text');
  });

  it('handles links', () => {
    const html = renderMarkdown('[link](https://example.com)');
    expect(html).toContain('<a href="https://example.com"');
    expect(html).toContain('link</a>');
  });

  describe('HTML sanitization', () => {
    it('escapes raw <iframe> in text instead of rendering it', () => {
      const html = renderMarkdown('Sandboxed <iframe> rendering');
      expect(html).not.toContain('<iframe>');
      expect(html).toContain('&lt;iframe&gt;');
    });

    it('escapes <script> tags', () => {
      const html = renderMarkdown('Try <script>alert(1)</script> here');
      expect(html).not.toContain('<script>');
      expect(html).toContain('&lt;script&gt;');
    });

    it('escapes <object> and <embed> tags', () => {
      const html = renderMarkdown('Use <object data="x"> and <embed src="y">');
      expect(html).not.toContain('<object');
      expect(html).not.toContain('<embed');
      expect(html).toContain('&lt;object');
      expect(html).toContain('&lt;embed');
    });

    it('does not affect HTML inside code blocks', () => {
      const html = renderMarkdown('```\n<iframe src="x"></iframe>\n```');
      expect(html).toContain('code-block-wrapper');
      expect(html).not.toContain('<iframe');
      expect(html).toContain('&lt;iframe');
    });

    it('escapes structural HTML tags inside code blocks so they render as text', () => {
      // Regression: <html>, <head>, <body>, <title>, <!DOCTYPE> are not in the
      // DANGEROUS_TAG filter. If the code renderer doesn't escape its text,
      // the browser parses these as actual elements and the code block renders
      // empty (the user-reported bug for the JS SDK boilerplate snippet).
      const md = '```html\n<!DOCTYPE html>\n<html>\n  <head>\n    <title>X</title>\n  </head>\n  <body>hi</body>\n</html>\n```';
      const html = renderMarkdown(md);
      expect(html).not.toContain('<!DOCTYPE html>');
      expect(html).not.toContain('<html>');
      expect(html).not.toContain('<title>');
      expect(html).not.toContain('<body>');
      expect(html).toContain('&lt;!DOCTYPE html&gt;');
      expect(html).toContain('&lt;title&gt;X&lt;/title&gt;');
      expect(html).toContain('&lt;body&gt;hi&lt;/body&gt;');
    });

    it('preserves HTML inside inline code', () => {
      const html = renderMarkdown('use `<iframe>` element');
      expect(html).toContain('<code>&lt;iframe&gt;</code>');
    });
  });

  describe('copy blocks', () => {
    it('renders inline copy block with wrapper and button', () => {
      const html = renderMarkdown('Call <copy>+1-555-0123</copy> for info');
      expect(html).toContain('class="copyable-block"');
      expect(html).toContain('data-copy-text="+1-555-0123"');
      expect(html).toContain('class="copy-btn"');
      expect(html).toContain('+1-555-0123');
    });

    it('renders multiline copy block with multi class', () => {
      const html = renderMarkdown('<copy>line one\nline two</copy>');
      expect(html).toContain('copyable-block-multi');
      expect(html).toContain('data-copy-text="line one&#10;line two"');
    });

    it('uses span for inline and div for multiline', () => {
      const inlineHtml = renderMarkdown('<copy>short</copy>');
      expect(inlineHtml).toContain('<span class="copyable-block"');

      const multiHtml = renderMarkdown('<copy>a\nb</copy>');
      expect(multiHtml).toContain('<div class="copyable-block copyable-block-multi"');
    });

    it('encodes special characters in data attribute', () => {
      const html = renderMarkdown('<copy>a & "b"</copy>');
      expect(html).toContain('data-copy-text="a &amp; &quot;b&quot;"');
    });

    it('renders markdown inside copy blocks', () => {
      const html = renderMarkdown('<copy>**bold** text</copy>');
      expect(html).toContain('<strong>bold</strong>');
    });

    it('renders large multiline copy block without breaking HTML structure', () => {
      // Regression: long multiline <copy> blocks had raw newlines in data-copy-text
      // attribute, which broke marked's HTML parser. The SVG copy icon would render
      // at full container size instead of being constrained inside .copy-btn.
      const content = `Refactor panel overlay state from 6 independent signals into a single discriminated union.

Current state: currentSkill, currentComponent, previewFile, panelUrl, activeInlineForm, and viewingNotification are 6 separate signals in store/store.ts that represent mutually exclusive panel overlay state. They're checked in a priority chain in ContentPane.tsx and must be cleared together in switchMenuItem(). This design allows invalid states and has caused bugs (see commit 57eaf3d1 — overlay not clearing when activeMenuItem unchanged).

Target state: Replace with a single signal:

type PanelOverlay =
  | { type: 'form'; form: InlineForm }
  | { type: 'skill-ui'; skill: Skill; component: SkillUiComponent }
  | { type: 'file-preview'; path: string }
  | { type: 'url-preview'; url: string }
  | { type: 'notification-detail'; notification: Notification }
  | null;

Key files to change:
- store/store.ts — replace 6 signals with one panelOverlay signal
- store/actions/menu.ts — clearing becomes panelOverlay.value = null
- store/actions/navigation.ts — NavEntry stores one overlay value instead of 6 fields
- store/actions/skills.ts — openSkill(), openSkillUi(), closeSkillWindow() set/clear the union
- store/actions/artifacts.ts — openUrl(), file preview setters
- store/actions/notifications.ts — viewNotification()
- components/layout/ContentPane.tsx — replace priority chain with switch on overlay.type
- components/layout/AppHeader.tsx — reads overlay to determine header title
- ~25 files total touch these signals

Constraints:
- Write integration tests BEFORE refactoring
- Migrate the existing menu.test.ts tests to use the new union
- Navigation save/restore in localStorage must remain backward-compatible
- npm test and npx tsc --noEmit must pass with zero errors`;
      const html = renderMarkdown(`<copy>${content}</copy>`);

      // The copy-btn must contain the SVG — not be broken out of its wrapper
      expect(html).toContain('class="copy-btn"');
      expect(html).toContain('copyable-block-multi');

      // The SVG must be INSIDE the button, not floating loose
      const btnStart = html.indexOf('class="copy-btn"');
      const svgStart = html.indexOf('<svg');
      const btnEnd = html.indexOf('</button>');
      expect(btnStart).toBeGreaterThan(-1);
      expect(svgStart).toBeGreaterThan(btnStart);
      expect(btnEnd).toBeGreaterThan(svgStart);

      // Content must be visible (not swallowed by broken HTML)
      expect(html).toContain('Refactor panel overlay');
      expect(html).toContain('Key files to change');

      // No loose SVGs outside of button wrappers
      const svgCount = (html.match(/<svg/g) || []).length;
      const btnSvgCount = (html.match(/class="copy-btn"[^>]*>[\s]*<svg/g) || []).length;
      expect(svgCount).toBe(btnSvgCount);
    });

    it('multiline copy block with markdown headings and code fences survives marked', () => {
      // Regression: marked's HTML block rules end a <div> at the first blank line.
      // A multiline copy block wrapping markdown content (headings, code fences)
      // gets its wrapper div broken — the copy button ends up orphaned.
      const content = `# Advanced Prompt

Here is an example:

\`\`\`rust
fn main() {
    println!("hello");
}
\`\`\`

Use this pattern for all prompts.`;

      const html = renderMarkdown(`<copy>${content}</copy>`);

      // The wrapper div must contain the copy button
      expect(html).toContain('copyable-block-multi');

      // The copy button must exist and contain the SVG
      const btnMatch = html.match(/<button[^>]*class="copy-btn"[^>]*>[\s\S]*?<\/button>/);
      expect(btnMatch).not.toBeNull();

      // The wrapper div must close AFTER the button, not before the content
      const wrapperStart = html.indexOf('copyable-block-multi');
      const btnPos = html.indexOf('class="copy-btn"');
      expect(wrapperStart).toBeGreaterThan(-1);
      expect(btnPos).toBeGreaterThan(wrapperStart);

      // Content must be rendered as markdown inside the wrapper
      expect(html).toContain('<h1');
      expect(html).toContain('Advanced Prompt');
      expect(html).toContain('println!');

      // data-copy-text must contain the raw text for clipboard
      expect(html).toContain('data-copy-text=');
    });

    it('does not match <copy> tags inside backtick code spans', () => {
      // When the LLM writes `<copy>` (backtick-quoted tag reference),
      // the preprocessor must not treat it as a real copy block start.
      const html = renderMarkdown('I understand the `<copy>` syntax.\n\n<copy>actual content</copy>');

      // The backtick-quoted <copy> must render as inline code, not trigger a copy block
      expect(html).toContain('<code>&lt;copy&gt;</code>');

      // The real copy block must still work
      expect(html).toContain('copyable-block');
      expect(html).toContain('actual content');

      // No orphaned backticks
      expect(html).not.toMatch(/I understand the `<br/);
    });

    it('does not match <copy> tags inside fenced code blocks', () => {
      const html = renderMarkdown('```\n<copy>not real</copy>\n```\n\n<copy>real</copy>');

      // Only one copy block (the real one), not the one inside the code fence
      const copyBlocks = html.match(/copyable-block/g);
      expect(copyBlocks).not.toBeNull();
      expect(copyBlocks!.length).toBe(1);

      // The fenced one should render as code
      expect(html).toContain('code-block-wrapper');
    });

    it('preserves inline code backticks in data-copy-text', () => {
      const html = renderMarkdown('Currently, <copy>`getTextContent()` only returns text content.</copy>');
      // data-copy-text must contain the backtick-wrapped code, not a placeholder
      expect(html).toContain('data-copy-text="`getTextContent()` only returns text content."');
      expect(html).not.toContain('CODE');
    });

    it('preserves inline code in multiline copy block data-copy-text', () => {
      const html = renderMarkdown('<copy>Run `npm install`\nthen `npm start`</copy>');
      expect(html).toContain('data-copy-text="Run `npm install`&#10;then `npm start`"');
      expect(html).not.toContain('CODE');
    });

    it('handles multiple copy blocks in one message', () => {
      const html = renderMarkdown('ID: <copy>abc</copy> and <copy>xyz</copy>');
      const matches = html.match(/copyable-block/g);
      // Each block has the class once in the element, so at least 2
      expect(matches!.length).toBeGreaterThanOrEqual(2);
    });
  });

  describe('renderMarkdownInline (phrasing-content-only)', () => {
    it('renders inline tokens — bold, italic, code, breaks', () => {
      const html = renderMarkdownInline('**bold** *it* `code` line\nbreak');
      expect(html).toContain('<strong>bold</strong>');
      expect(html).toContain('<em>it</em>');
      expect(html).toContain('<code>code</code>');
      expect(html).toContain('<br');
    });

    it('does NOT emit block elements — paragraphs, lists, headings stay as literal text', () => {
      // The whole point: the output must be safe to nest inside <button> /
      // <span>, so block markdown is left as-is rather than wrapped in
      // <p>/<ul>/<h*>. Otherwise we'd reintroduce the HTML-validity bug the
      // helper exists to avoid.
      const html = renderMarkdownInline('# heading\n- item one\n- item two');
      expect(html).not.toContain('<p>');
      expect(html).not.toContain('<ul>');
      expect(html).not.toContain('<li>');
      expect(html).not.toContain('<h1');
      // Dashes survive as visual bullets, content is preserved.
      expect(html).toContain('# heading');
      expect(html).toContain('- item one');
      expect(html).toContain('- item two');
    });

    it('strips <a> from markdown links but keeps the label text', () => {
      // <a> is interactive content; inside the AskUserQuestion option <button>
      // that's interactive-in-interactive (HTML spec violation). The renderer
      // returns label text only.
      const html = renderMarkdownInline('see [docs](https://example.com) for more');
      expect(html).not.toContain('<a ');
      expect(html).not.toContain('href');
      expect(html).toContain('see docs for more');
    });

    it('preserves nested inline markdown inside link text after stripping', () => {
      // Without parser.parseInline on the link's child tokens, **bold** inside
      // link text would survive as literal asterisks instead of <strong>.
      const html = renderMarkdownInline('[**bold link**](https://x)');
      expect(html).toContain('<strong>bold link</strong>');
      expect(html).not.toContain('<a ');
    });

    it('discards javascript:-scheme link targets along with the wrapper', () => {
      // Bonus property of dropping the href: dangerous URL schemes never reach
      // the DOM. The label text survives, the target does not.
      const html = renderMarkdownInline('[click me](javascript:alert(1))');
      expect(html).toContain('click me');
      expect(html).not.toContain('javascript:');
      expect(html).not.toContain('<a ');
    });

    it('still escapes dangerous tags from raw source', () => {
      const html = renderMarkdownInline('try <script>x</script> here');
      expect(html).not.toContain('<script>');
      expect(html).toContain('&lt;script&gt;');
    });

    it('handles empty input', () => {
      expect(renderMarkdownInline('')).toBe('');
    });
  });

  describe('renderMarkdownInlineWithLinks (phrasing content + live links)', () => {
    it('linkifies a bare URL into a new-tab anchor', () => {
      const html = renderMarkdownInlineWithLinks(
        'Draft PR #1488 is ready: https://github.com/m10s-green/user-acquisition/pull/1488 — mark it ready?',
      );
      expect(html).toContain(
        '<a href="https://github.com/m10s-green/user-acquisition/pull/1488" target="_blank" rel="noopener">',
      );
      expect(html).toContain('https://github.com/m10s-green/user-acquisition/pull/1488</a>');
    });

    it('keeps [label](url) markdown links as anchors with the label text', () => {
      const html = renderMarkdownInlineWithLinks('see [the PR](https://example.com/pr/1) please');
      expect(html).toContain('<a href="https://example.com/pr/1" target="_blank" rel="noopener">');
      expect(html).toContain('the PR</a>');
    });

    it('still renders inline markdown — bold, italic, code, breaks', () => {
      const html = renderMarkdownInlineWithLinks('**bold** *it* `code` line\nbreak');
      expect(html).toContain('<strong>bold</strong>');
      expect(html).toContain('<em>it</em>');
      expect(html).toContain('<code>code</code>');
      expect(html).toContain('<br');
    });

    it('does NOT emit block elements — stays safe as phrasing content', () => {
      const html = renderMarkdownInlineWithLinks('# heading\n- item one');
      expect(html).not.toContain('<p>');
      expect(html).not.toContain('<ul>');
      expect(html).not.toContain('<h1');
    });

    it('drops javascript:-scheme links to label text (no anchor, no scheme)', () => {
      const html = renderMarkdownInlineWithLinks('[click me](javascript:alert(1))');
      expect(html).toContain('click me');
      expect(html).not.toContain('<a ');
      expect(html).not.toContain('javascript:');
    });

    it('does not anchor relative or non-http schemes', () => {
      const html = renderMarkdownInlineWithLinks('[app](app:todo) and [file](data/artifacts/x.md)');
      expect(html).not.toContain('<a ');
      expect(html).toContain('app');
      expect(html).toContain('file');
    });

    it('escapes quotes in the href so the attribute cannot break out', () => {
      const html = renderMarkdownInlineWithLinks('[x](https://example.com/"onmouseover="alert(1))');
      expect(html).not.toContain('"onmouseover="');
      expect(html).toContain('&quot;');
    });

    it('still escapes dangerous tags from raw source', () => {
      const html = renderMarkdownInlineWithLinks('try <script>x</script> here');
      expect(html).not.toContain('<script>');
      expect(html).toContain('&lt;script&gt;');
    });

    it('handles empty input', () => {
      expect(renderMarkdownInlineWithLinks('')).toBe('');
    });
  });

  describe('thread reference links', () => {
    it('rewrites bare-UUID thread links into clickable thread chips', () => {
      const html = renderMarkdown('See [the bug](thread:1c2419a1-aaaa-bbbb-cccc-ddddeeeeffff)');
      expect(html).toContain('class="thread-link"');
      expect(html).toContain('data-thread-id="1c2419a1-aaaa-bbbb-cccc-ddddeeeeffff"');
      expect(html).not.toContain('href="thread:');
      expect(html).not.toContain('data-thread-workspace');
    });

    it('rewrites workspace-qualified thread links and preserves the workspace', () => {
      const html = renderMarkdown('See [the bug](thread:dev/1c2419a1-aaaa-bbbb-cccc-ddddeeeeffff)');
      expect(html).toContain('class="thread-link"');
      expect(html).toContain('data-thread-id="1c2419a1-aaaa-bbbb-cccc-ddddeeeeffff"');
      expect(html).toContain('data-thread-workspace="dev"');
      expect(html).not.toContain('href="thread:');
    });
  });
});
