import { describe, it, expect, vi, beforeEach, afterEach } from 'vitest';
import { marked } from 'marked';

// Mutable stand-in for basePath's load-time `WORKSPACE_ID` const (the gateway
// slug this bundle is served under, or null when served directly / in tests with
// no stamped <base>). Read via a getter so renderMarkdown sees the current value.
const base = vi.hoisted(() => ({ workspaceId: null as string | null }));
vi.mock('./basePath', () => ({
  get WORKSPACE_ID() {
    return base.workspaceId;
  },
}));

import { lucidos } from '@lucidos/sdk';
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

    it('strips event-handler attributes from raw HTML that marked preserves', () => {
      const html = renderMarkdown('<img src="x" onerror="alert(1)"> <span onClick=alert(2)>ok</span>');
      expect(html).toContain('<img src="x">');
      expect(html).toContain('<span>ok</span>');
      expect(html).not.toContain('onerror');
      expect(html).not.toContain('onClick');
      expect(html).not.toContain('alert(1)');
    });

    it('strips javascript and data URL attributes from raw HTML', () => {
      const html = renderMarkdown('<a href="javascript:alert(1)">x</a><img src=data:text/html,evil>');
      expect(html).toContain('<a>x</a>');
      expect(html).toContain('<img>');
      expect(html).not.toContain('javascript:');
      expect(html).not.toContain('data:text/html');
    });

    it('strips entity-obfuscated javascript and data URL attributes from raw HTML', () => {
      const html = renderMarkdown(
        '<a href="jav&#x61;script&colon;alert(1)">x</a><img src="da&#116;a:text/html,evil">',
      );
      expect(html).toContain('<a>x</a>');
      expect(html).toContain('<img>');
      expect(html).not.toContain('jav&#x61;script');
      expect(html).not.toContain('data:text/html');
      expect(html).not.toContain('alert(1)');
    });

    it('strips URL attributes with embedded control whitespace in dangerous schemes', () => {
      const html = renderMarkdown('<a href="java&#10;script:alert(1)">x</a><img src="da\tta:text/html,evil">');
      expect(html).toContain('<a>x</a>');
      expect(html).toContain('<img>');
      expect(html).not.toContain('java&#10;script');
      expect(html).not.toContain('data:text/html');
    });

    it('does not throw on out-of-range numeric character references in URL attributes', () => {
      expect(() => renderMarkdown('<a href="&#99999999;">x</a>')).not.toThrow();
    });

    // The attribute scrubbers are shaped like `\s+name=value`, and prose
    // contains that shape too. Run over the whole rendered string they deleted
    // the user's own words with no sign anything had gone: "Set online=yes in
    // the config" came out as "Set in the config", and a line listing
    // `on_error=` / `on_success=` came out as the bare "Values:" label. They
    // are scoped to tag regions now (`scrubTagAttributes`, walked from
    // `sanitizeHtmlFragments`).
    it.each([
      ['Set online=yes in the config file.', 'Set online=yes in the config file.'],
      ['Use once=true to run it a single time.', 'Use once=true to run it a single time.'],
      ['Deploy with only=web and once=1.', 'Deploy with only=web and once=1.'],
      ['Values: on_error=retry, on_success=stop.', 'Values: on_error=retry, on_success=stop.'],
      ['The flag onboarding=disabled turns it off.', 'The flag onboarding=disabled turns it off.'],
    ])('keeps prose that merely looks like a handler attribute: %s', (src, kept) => {
      expect(renderMarkdown(src, { cache: false })).toContain(kept);
    });

    it('keeps an `on…=` run inside a copy block, so the copied text is intact', () => {
      const html = renderMarkdown('<copy>lucidos trigger set on_event=ThreadCompleted</copy>', { cache: false });
      expect(html).toContain('data-copy-text="lucidos trigger set on_event=ThreadCompleted"');
    });

    // Attribute values may contain `>`, so a tag scan that stopped at the first
    // one would leave everything after it outside any tag region and therefore
    // unscrubbed. That would turn the prose fix above into an XSS hole.
    it('still strips a handler/href hidden behind a `>` inside an earlier attribute value', () => {
      const html = renderMarkdown(
        '<a title="a>b" href="javascript:alert(1)" onmouseover="alert(2)">x</a>',
        { cache: false },
      );
      expect(html).not.toContain('javascript:');
      expect(html).not.toContain('onmouseover');
      expect(html).not.toContain('alert(');
    });

    it('leaves an unterminated tag inert: marked escapes it to text', () => {
      const html = renderMarkdown('<a href="javascript:alert(1)" onclick="alert(2)"', { cache: false });
      expect(html).not.toContain('<a ');
      expect(html).toContain('&lt;a href=&quot;javascript:alert(1)&quot;');
    });

    it('drops a handler attribute without leaving a double space behind', () => {
      expect(renderMarkdown('<span class="a" onclick="x" id="b">y</span>', { cache: false }))
        .toContain('<span class="a" id="b">y</span>');
    });

    it('keeps a bare boolean attribute and the attribute after it', () => {
      expect(renderMarkdown('<input disabled onfocus="x" value="v">', { cache: false }))
        .toContain('<input disabled value="v">');
    });

    // A quote only delimits an attribute VALUE, so it counts only where a value
    // is expected. Treating every `'` as one reopened the prose-deletion bug
    // through a different door: an apostrophe in an HTML comment, or in an
    // unquoted attribute value, opened a quote that never closed, so the tag
    // region ran to the end of the document and the scrub walked plain prose.
    it('keeps prose after an HTML comment containing an apostrophe', () => {
      const html = renderMarkdown("<!-- don't forget -->\n\nSet online=yes in the config.", { cache: false });
      expect(html).toContain('online=yes');
    });

    it('keeps prose after an unquoted attribute value containing an apostrophe', () => {
      const html = renderMarkdown("<h2 id=it's>T</h2>\n\nSet online=yes in the config.", { cache: false });
      expect(html).toContain('online=yes');
    });

    it('keeps inline prose after an inline HTML comment with an apostrophe', () => {
      const html = renderMarkdownInline("Inline <!-- don't --> then online=yes stays");
      expect(html).toContain('online=yes');
    });

    // RCDATA is where the sanitizer and the browser disagree about what a
    // comment IS. Inside `<textarea>` / `<title>` a `<!--` is plain text to the
    // browser and `</textarea>` still closes the element, so treating the rest
    // of the document as comment body and passing it through would hand back a
    // live handler. The comment region is BOUNDED (so prose after a real comment
    // survives) but still scrubbed (so this cannot become an escape hatch).
    it.each([
      '<textarea><!--</textarea><img src=x onerror=alert(1)>',
      '<title><!--</title><img src=x onerror=alert(1)>',
      '<textarea rows=1><!--</textarea><a href="javascript:alert(1)">click</a>',
    ])('does not let an unterminated comment in RCDATA smuggle a handler: %s', (src) => {
      const html = renderMarkdown(src, { cache: false });
      expect(html).not.toMatch(/onerror/i);
      expect(html).not.toMatch(/javascript:/i);
    });

    // The two XSS guarantees the quote handling exists for must survive the
    // narrowing: a `>` hidden in a quoted value, and an unquoted handler.
    it('still strips an unquoted handler attribute', () => {
      const html = renderMarkdown('<img src=x onerror=alert(1)>', { cache: false });
      expect(html).not.toContain('onerror');
      expect(html).not.toContain('alert(');
    });

    // A raw-text element stops the browser tokenizing tags until its end tag,
    // so a scan that keeps tokenizing lands a live element inside what it
    // believes is a quoted attribute value and never sees its handler.
    it.each([
      '<textarea><a title="</textarea><img src=x onerror="alert(1)">',
      "<title><a title='</title><img src=x onerror='alert(1)'>",
      '<textarea><a title="</textarea><a href="javascript:alert(1)">go</a>',
      'Here is a form:\n\n<textarea><!--<b c="</textarea><img src=x onerror=alert(1)>\n\nDone.',
    ])('keeps a raw-text element from smuggling a live element past the walk: %s', (src) => {
      const html = renderMarkdown(src, { cache: false });
      // Everything between the start and end tag is inert text, so the only
      // thing that matters is that nothing live survives AFTER the end tag.
      const after = html.slice(html.toLowerCase().lastIndexOf('</textarea>') + 1)
        + html.slice(html.toLowerCase().lastIndexOf('</title>') + 1);
      expect(after).not.toMatch(/<img[^>]*onerror/i);
      expect(after).not.toMatch(/<a[^>]*javascript:/i);
    });

    it('resumes scrubbing after a raw-text element closes', () => {
      const html = renderMarkdown('<textarea>x</textarea><img src=y onerror=alert(1)>', { cache: false });
      expect(html).not.toMatch(/onerror/i);
    });

    // The region is bounded at the end tag but still scrubbed, because `title`
    // is RCDATA in HTML and ordinary markup inside `<svg>` / `<math>`, and this
    // walk does not track foreign content. The visible cost is that literal
    // markup typed into a textarea loses its handler attributes.
    it('scrubs inside a raw-text element rather than trusting it as text', () => {
      const html = renderMarkdown('<textarea><b onclick="x">hi</b></textarea>', { cache: false });
      expect(html).not.toContain('onclick');
      expect(html).toContain('hi');
    });

    it('scrubs a handler inside an SVG title, which is markup and not RCDATA', () => {
      const html = renderMarkdown('<svg><title><b onclick="alert(1)">t</b></title></svg>', { cache: false });
      expect(html).not.toContain('onclick');
    });

    // SMIL reaches a URL by indirection, which a name-based attribute filter
    // cannot see, so the element itself is escaped.
    it('neutralizes an SVG animate that targets href', () => {
      const html = renderMarkdown(
        '<svg><a id=x><animate attributeName="href" values="javascript:alert(1)" begin="0s" fill="freeze"/><text>go</text></a></svg>',
        { cache: false },
      );
      expect(html).not.toMatch(/<animate/i);
    });

    it.each(['poster', 'srcset', 'background', 'ping', 'data'])(
      'strips a dangerous URL from the %s attribute',
      (attr) => {
        const html = renderMarkdown(`<div ${attr}="javascript:alert(1)">x</div>`, { cache: false });
        expect(html).not.toMatch(/javascript:/i);
      },
    );

    // The SVG spelling of href navigates identically, and a form submits to its
    // action, so all three take a `javascript:` URL the same way `href` does.
    it.each([
      '<svg><a xlink:href="javascript:alert(1)"><text>click</text></a></svg>',
      '<form action="javascript:alert(1)"><button>go</button></form>',
      '<button formaction="javascript:alert(1)">go</button>',
    ])('strips a dangerous URL from every spelling of a navigating attribute: %s', (src) => {
      expect(renderMarkdown(src, { cache: false })).not.toMatch(/javascript:/i);
    });

    it('keeps an ordinary action and xlink:href untouched', () => {
      const html = renderMarkdown('<svg><a xlink:href="/docs">d</a></svg>', { cache: false });
      expect(html).toContain('xlink:href="/docs"');
    });

    // marked escapes this one to text outright (the apostrophe stops it being
    // parsed as raw HTML), so the guarantee is "no live tag carries a handler",
    // not "the substring is absent".
    it('still strips a handler after an unquoted value containing an apostrophe', () => {
      const html = renderMarkdown("<img alt=it's onerror=\"alert(1)\">", { cache: false });
      expect(html).not.toMatch(/<img[^>]*onerror/i);
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

    it('strips raw inline HTML event handlers', () => {
      const html = renderMarkdownInline('<span onclick="alert(1)">tap</span>');
      expect(html).toContain('<span>tap</span>');
      expect(html).not.toContain('onclick');
      expect(html).not.toContain('alert(1)');
    });

    it('handles empty input', () => {
      expect(renderMarkdownInline('')).toBe('');
    });
  });

  describe('renderMarkdownInlineWithLinks (phrasing content + live links)', () => {
    it('linkifies a bare URL into a new-tab anchor', () => {
      const html = renderMarkdownInlineWithLinks(
        'Draft PR #1488 is ready: https://github.com/example-org/example-repo/pull/1488 — mark it ready?',
      );
      expect(html).toContain(
        '<a href="https://github.com/example-org/example-repo/pull/1488" target="_blank" rel="noopener">',
      );
      expect(html).toContain('https://github.com/example-org/example-repo/pull/1488</a>');
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

    it('strips raw inline HTML javascript URLs', () => {
      const html = renderMarkdownInlineWithLinks('<a href="javascript:alert(1)">tap</a>');
      expect(html).toContain('<a>tap</a>');
      expect(html).not.toContain('javascript:');
      expect(html).not.toContain('alert(1)');
    });

    it('handles empty input', () => {
      expect(renderMarkdownInlineWithLinks('')).toBe('');
    });
  });

  describe('thread reference links', () => {
    // No <base> stamped in tests → WORKSPACE_ID null → served-directly fallback.
    beforeEach(() => {
      base.workspaceId = null;
    });

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

    it('uses href="#" when not served behind the gateway (no workspace slug)', () => {
      const html = renderMarkdown('See [it](thread:0a11aaaa-bbbb-cccc-dddd-eeeeffff0009)');
      expect(html).toContain('href="#"');
      expect(html).toContain('class="thread-link"');
    });
  });

  describe('thread reference links — behind the gateway', () => {
    // Served at https://<gateway>/myws/ → hover should show the real
    // destination, not the `#`-resolves-to-current-page URL.
    beforeEach(() => {
      base.workspaceId = 'myws';
      vi.stubGlobal('location', { origin: 'https://localhost:5251' });
    });
    afterEach(() => {
      vi.unstubAllGlobals();
      base.workspaceId = null;
    });

    it('points an untagged (same-workspace) link at the current workspace slug', () => {
      const html = renderMarkdown('See [the bug](thread:aa11aaaa-bbbb-cccc-dddd-eeeeffff0001)');
      expect(html).toContain('href="https://localhost:5251/myws/#thread=aa11aaaa-bbbb-cccc-dddd-eeeeffff0001"');
      expect(html).toContain('class="thread-link"');
    });

    it('points a cross-workspace link at the target workspace slug', () => {
      const html = renderMarkdown('See [the bug](thread:dev/aa11aaaa-bbbb-cccc-dddd-eeeeffff0002)');
      expect(html).toContain('href="https://localhost:5251/dev/#thread=aa11aaaa-bbbb-cccc-dddd-eeeeffff0002"');
      expect(html).toContain('data-thread-workspace="dev"');
    });

    it('slugifies the ref workspace name for the href (lowercased)', () => {
      const html = renderMarkdown('See [the bug](thread:Dev/aa11aaaa-bbbb-cccc-dddd-eeeeffff0003)');
      expect(html).toContain('href="https://localhost:5251/dev/#thread=aa11aaaa-bbbb-cccc-dddd-eeeeffff0003"');
      // raw (un-slugified) workspace name is preserved for the click handler
      expect(html).toContain('data-thread-workspace="Dev"');
    });
  });

  // Regression guard for the "heavy thread freezes on send" fix: re-rendering a
  // thread re-calls renderMarkdown for every block; without caching, each
  // re-render re-parsed all of them (the markdown re-parse storm). The cache
  // makes a repeated input an O(1) lookup; the live streaming buffer opts out.
  describe('parse caching', () => {
    it('caches by input so a repeated render does not re-parse', () => {
      const spy = vi.spyOn(marked, 'parse');
      const md = '**cache-hit-unique-marker-alpha** with `code` and a list\n- one\n- two';
      const before = spy.mock.calls.length;
      const a = renderMarkdown(md);
      const b = renderMarkdown(md);
      expect(b).toBe(a);
      expect(spy.mock.calls.length).toBe(before + 1); // parsed once; second was a cache hit
      spy.mockRestore();
    });

    it('cache:false bypasses the cache (streaming buffer never pollutes it)', () => {
      const spy = vi.spyOn(marked, 'parse');
      const md = '**cache-bypass-unique-marker-beta** streaming fragment';
      const before = spy.mock.calls.length;
      renderMarkdown(md, { cache: false });
      renderMarkdown(md, { cache: false });
      expect(spy.mock.calls.length).toBe(before + 2); // re-parsed both times, not cached
      spy.mockRestore();
    });
  });
});

/**
 * Images. Two things happen to an `<img>` the block renderer emits: a
 * workspace-relative `src` is resolved against the mount that actually serves
 * the file, and the tag is wrapped in the `.image-scroll-wrapper` scroll
 * container. The URL is built by the SDK's `lucidos.data.url`, so these tests
 * drive the served context the way the browser supplies it: `configure({
 * baseUrl })` for the gateway's `/<slug>` prefix (absent on a bare engine port)
 * plus a stubbed `location`, since the SDK reads `window.location` to tell an
 * app iframe from the host shell.
 *
 * `cache: false` throughout, because renderMarkdown memoizes by input text
 * alone: two served contexts rendering the same markdown would otherwise share
 * one result.
 */
describe('renderMarkdown images', () => {
  const servedAt = (baseUrl: string, pathname: string) => {
    lucidos.configure({ baseUrl });
    vi.stubGlobal('location', { origin: 'https://localhost:5251', pathname, search: '' });
  };
  const imgSrc = (html: string) => html.match(/<img[^>]*\ssrc="([^"]*)"/)?.[1];

  beforeEach(() => {
    base.workspaceId = 'myws';
    servedAt('/myws', '/myws/');
  });

  afterEach(() => {
    vi.unstubAllGlobals();
    lucidos.configure({ baseUrl: '' });
    base.workspaceId = null;
  });

  it('resolves a workspace-relative source through the data mount', () => {
    const html = renderMarkdown('![right aligned](artifacts/screenshots/hero.png)', { cache: false });
    expect(imgSrc(html)).toBe('/myws/data/artifacts/screenshots/hero.png');
  });

  it('carries no workspace prefix when served directly on an engine port', () => {
    base.workspaceId = null;
    servedAt('', '/');
    const html = renderMarkdown('![alt](artifacts/screenshots/hero.png)', { cache: false });
    expect(imgSrc(html)).toBe('/data/artifacts/screenshots/hero.png');
  });

  it('resolves the other workspace directories the same way', () => {
    for (const dir of ['apps', 'knowhow', 'triggers']) {
      const html = renderMarkdown(`![alt](${dir}/thing/logo.png)`, { cache: false });
      expect(imgSrc(html)).toBe(`/myws/data/${dir}/thing/logo.png`);
    }
  });

  it('routes system-knowhow through the API endpoint, not the static mount', () => {
    // system-knowhow ships with the engine rather than living in the workspace,
    // so it is not under the static /data mount. Deferring to the SDK's builder
    // is what gets this right without renderMarkdown knowing about the case.
    const html = renderMarkdown('![diagram](system-knowhow/diagram.png)', { cache: false });
    expect(imgSrc(html)).toBe('/myws/api/v1/data/system-knowhow/diagram.png');
  });

  it('encodes a source exactly once', () => {
    // marked percent-encodes the src it emits and the SDK encodes each segment
    // again, so a non-ASCII name would round-trip to `%C3%83%C2%A6…` and 404.
    const html = renderMarkdown('![alt](artifacts/æøå.png)', { cache: false });
    expect(imgSrc(html)).toBe('/myws/data/artifacts/%C3%A6%C3%B8%C3%A5.png');
  });

  it('leaves an absolute URL untouched, but still wraps it', () => {
    const html = renderMarkdown('![alt](https://example.com/x.png)', { cache: false });
    expect(imgSrc(html)).toBe('https://example.com/x.png');
    expect(html).toContain('<span class="image-scroll-wrapper">');
  });

  it('leaves an already-absolute site path untouched', () => {
    const html = renderMarkdown('![alt](/already/absolute.png)', { cache: false });
    expect(imgSrc(html)).toBe('/already/absolute.png');
  });

  it('leaves a protocol-relative source untouched', () => {
    const html = renderMarkdown('![alt](//cdn.example.com/x.png)', { cache: false });
    expect(imgSrc(html)).toBe('//cdn.example.com/x.png');
  });

  it('leaves a relative path outside the workspace allowlist alone', () => {
    // The rule is an allowlist of workspace top-level directories rather than
    // "every relative path", so a relative asset path that means something else
    // in its own context is never silently redirected at the workspace.
    const html = renderMarkdown('![alt](foo/bar.png)', { cache: false });
    expect(imgSrc(html)).toBe('foo/bar.png');
  });

  it('refuses a source that walks out of the workspace', () => {
    const html = renderMarkdown('![alt](artifacts/../../etc/passwd)', { cache: false });
    expect(imgSrc(html)).toBe('artifacts/../../etc/passwd');
    expect(html).not.toContain('/data/');
  });

  it('refuses a percent-encoded traversal too', () => {
    const html = renderMarkdown('![alt](artifacts/%2e%2e/%2e%2e/etc/passwd)', { cache: false });
    expect(imgSrc(html)).toBe('artifacts/%2e%2e/%2e%2e/etc/passwd');
    expect(html).not.toContain('/data/');
  });

  it('refuses a traversal hidden behind an encoded separator', () => {
    // `%2e%2e%2f%2e%2e` decodes to a SINGLE segment reading `../..`, which a
    // check on the pre-join segments would wave through. The path encoder then
    // splits it back into two real segments (a dot is unreserved, so nothing
    // re-escapes them) and the browser normalizes the request out of the mount.
    const html = renderMarkdown('![alt](artifacts/%2e%2e%2f%2e%2e/api/v1/health)', { cache: false });
    expect(imgSrc(html)).toBe('artifacts/%2e%2e%2f%2e%2e/api/v1/health');
    expect(html).not.toContain('/data/');
  });

  it('keeps a query string a query rather than folding it into the file name', () => {
    const html = renderMarkdown('![alt](artifacts/x.png?v=2)', { cache: false });
    expect(imgSrc(html)).toBe('/myws/data/artifacts/x.png?v=2');
  });

  it('does not re-escape an ampersand the author already escaped', () => {
    // The query is carried over verbatim from an attribute value that is
    // already escaped. Escaping it a second time would send the server the
    // literal text `&amp;` as a parameter name.
    const html = renderMarkdown('![alt](artifacts/x.png?a=1&amp;b=2)', { cache: false });
    expect(imgSrc(html)).toBe('/myws/data/artifacts/x.png?a=1&amp;b=2');
    expect(html).not.toContain('&amp;amp;');
  });

  it('keeps a fragment attached after the path', () => {
    const html = renderMarkdown('![alt](artifacts/chart.svg#detail)', { cache: false });
    expect(imgSrc(html)).toBe('/myws/data/artifacts/chart.svg#detail');
  });

  it('still strips a data: image URI, exactly as before the rewrite existed', () => {
    const html = renderMarkdown('![alt](data:image/png;base64,AAAA)', { cache: false });
    expect(html).not.toContain('data:image/png');
    expect(html).toContain('<img alt="alt">');
  });

  it('wraps the image in the scroll container with the rewrite applied inside it', () => {
    const html = renderMarkdown('![right aligned](artifacts/screenshots/hero.png)', { cache: false });
    expect(html).toContain(
      '<span class="image-scroll-wrapper">'
      + '<img src="/myws/data/artifacts/screenshots/hero.png" alt="right aligned">'
      + '</span>',
    );
  });

  it('gives two images in one document a wrapper each', () => {
    const html = renderMarkdown('![a](artifacts/one.png)\n\n![b](artifacts/two.png)', { cache: false });
    expect(html.match(/<span class="image-scroll-wrapper">/g)).toHaveLength(2);
    expect(html).toContain('src="/myws/data/artifacts/one.png"');
    expect(html).toContain('src="/myws/data/artifacts/two.png"');
  });

  it('leaves the anchor structure of a linked image intact', () => {
    const html = renderMarkdown('[![alt](artifacts/x.png)](https://example.com)', { cache: false });
    expect(html).toContain(
      '<a href="https://example.com">'
      + '<span class="image-scroll-wrapper"><img src="/myws/data/artifacts/x.png" alt="alt"></span>'
      + '</a>',
    );
  });

  it('rewrites an inline-variant image but does NOT wrap it', () => {
    // The wrapper is a block scroll container, and both inline helpers must stay
    // phrasing content that nests inside a <button> or a <span>. An image in a
    // question or an option label is small by nature, so there is nothing to pan.
    for (const render of [renderMarkdownInline, renderMarkdownInlineWithLinks]) {
      const html = render('see ![alt](artifacts/x.png) here');
      expect(imgSrc(html)).toBe('/myws/data/artifacts/x.png');
      expect(html).not.toContain('image-scroll-wrapper');
    }
  });

  it('transforms a table and an image in the same document without corrupting either', () => {
    const md = [
      '| A | B | C | D |',
      '| --- | --- | --- | --- |',
      '| w | x | y | z |',
      '',
      '![alt](artifacts/x.png)',
    ].join('\n');
    const html = renderMarkdown(md, { cache: false });
    expect(html).toContain('<div class="table-scroll-wrapper"><table data-stack>');
    expect(html).toContain('<td data-label="A">w</td>');
    expect(html).toContain('<span class="image-scroll-wrapper">');
    expect(imgSrc(html)).toBe('/myws/data/artifacts/x.png');
  });

  it('wraps and rewrites an image sitting inside a table cell', () => {
    const md = ['| A | B |', '| --- | --- |', '| ![alt](artifacts/x.png) | y |'].join('\n');
    const html = renderMarkdown(md, { cache: false });
    expect(html).toContain('<div class="table-scroll-wrapper"><table>');
    expect(html).toContain(
      '<td><span class="image-scroll-wrapper">'
      + '<img src="/myws/data/artifacts/x.png" alt="alt">'
      + '</span></td>',
    );
  });
});

/**
 * Tables. Two properties are pinned here; the third (that a column is never
 * laid out narrower than its longest word) is a LAYOUT property with no
 * observable in this suite, since vitest runs against the stub `document` in
 * test-setup.ts with no layout engine. It lives in
 * e2e/markdown-table-columns.spec.ts instead.
 */
describe('renderMarkdown tables', () => {
  const mdRow = (cells: string[]) => `| ${cells.join(' | ')} |`;
  const mdTable = (headers: string[], ...rows: string[][]) =>
    [mdRow(headers), mdRow(headers.map(() => '---')), ...rows.map(mdRow)].join('\n');
  const labels = (html: string) =>
    [...html.matchAll(/data-label="([^"]*)"/g)].map((m) => m[1]);

  it('keeps the grid below the stack threshold', () => {
    for (const cols of [2, 3]) {
      const headers = Array.from({ length: cols }, (_, i) => `H${i}`);
      const html = renderMarkdown(mdTable(headers, headers.map((_, i) => `v${i}`)));
      expect(html).toContain('<div class="table-scroll-wrapper"><table>');
      expect(html).not.toContain('data-stack');
      expect(html).not.toContain('data-label');
    }
  });

  it('stacks at and above the threshold, labelling every cell', () => {
    for (const cols of [4, 5]) {
      const headers = Array.from({ length: cols }, (_, i) => `H${i}`);
      const html = renderMarkdown(mdTable(headers, headers.map((_, i) => `v${i}`)));
      expect(html).toContain('<div class="table-scroll-wrapper"><table data-stack>');
      expect(html).toContain('</table></div>');
      expect(labels(html)).toEqual(headers);
    }
  });

  it('restarts the labels on every row', () => {
    const html = renderMarkdown(
      mdTable(['A', 'B', 'C', 'D'], ['1', '2', '3', '4'], ['5', '6', '7', '8'])
    );
    expect(labels(html)).toEqual(['A', 'B', 'C', 'D', 'A', 'B', 'C', 'D']);
  });

  it('pairs each label with its own cell, not the next one', () => {
    const html = renderMarkdown(mdTable(['A', 'B', 'C', 'D'], ['w', 'x', 'y', 'z']));
    expect(html).toContain('<td data-label="A">w</td>');
    expect(html).toContain('<td data-label="D">z</td>');
  });

  it('keeps alignment attributes marked emits on a cell', () => {
    const md = [
      '| A | B | C | D |',
      '| :-- | :-: | --: | --- |',
      '| w | x | y | z |',
    ].join('\n');
    const html = renderMarkdown(md);
    expect(html).toContain('<td align="center" data-label="B">x</td>');
  });

  it('escapes a header so it cannot break out of the attribute', () => {
    const html = renderMarkdown(mdTable(['a"b', 'A & B', 'C', 'D'], ['w', 'x', 'y', 'z']));
    expect(html).toContain('data-label="a&quot;b"');
    // Single-escaped: a double escape would render the literal text "&amp;".
    expect(html).toContain('data-label="A &amp; B"');
    expect(html).not.toContain('&amp;amp;');
  });

  it('reduces a header carrying markup to its plain text', () => {
    const html = renderMarkdown(mdTable(['`code`', '**bold**', '[l](https://e.com)', 'D'], ['w', 'x', 'y', 'z']));
    expect(labels(html)).toEqual(['code', 'bold', 'l', 'D']);
    for (const label of labels(html)) expect(label).not.toContain('<');
  });

  it('drops an inline HTML tag in a header rather than labelling with it', () => {
    const html = renderMarkdown(mdTable(['<img src=x>', 'B', 'C', 'D'], ['w', 'x', 'y', 'z']));
    expect(labels(html)).toEqual(['', 'B', 'C', 'D']);
  });

  it('labels the padded cells of a short row, which stay empty for the CSS to hide', () => {
    // marked pads a short row out to the header's column count.
    const md = ['| A | B | C | D |', '| --- | --- | --- | --- |', '| w | x |'].join('\n');
    const html = renderMarkdown(md);
    expect(labels(html)).toEqual(['A', 'B', 'C', 'D']);
    expect(html).toContain('<td data-label="C"></td>');
  });

  // The four table regexes are module-level and carry /g, so a `lastIndex`
  // left behind by one call would corrupt the next. `replace` self-resets and
  // `matchAll` clones, so today they are safe; this pins that, because
  // switching one to `exec`/`test` would silently break the SECOND render.
  it('is stable across repeated renders of the same table', () => {
    const md = mdTable(['A', 'B', 'C', 'D'], ['w', 'x', 'y', 'z']);
    const first = renderMarkdown(md, { cache: false });
    const second = renderMarkdown(md, { cache: false });
    expect(second).toBe(first);
    expect(labels(second)).toEqual(['A', 'B', 'C', 'D']);
  });

  it('transforms every table in a document independently', () => {
    const wide = mdTable(['A', 'B', 'C', 'D'], ['w', 'x', 'y', 'z']);
    const narrow = mdTable(['E', 'F'], ['1', '2']);
    const html = renderMarkdown(`${wide}\n\n${narrow}`);
    expect(html.match(/<div class="table-scroll-wrapper">/g)).toHaveLength(2);
    expect(html.match(/<table data-stack>/g)).toHaveLength(1);
    expect(labels(html)).toEqual(['A', 'B', 'C', 'D']);
  });
});
