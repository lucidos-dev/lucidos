// @vitest-environment jsdom
//
// A real DOM, because the defect lives in the gap between the HTML serializer
// and the tag splitter. The serializer writes `<` and `>` raw inside an
// attribute value, and the splitter must not read a tag boundary there.
import { describe, expect, it } from 'vitest';
import { linkifyPaths } from './linkifyPaths';
import { renderMarkdown } from './renderMarkdown';

/** Every attribute name on every element of `html`, parsed the way the host
 *  document parses it when the chat assigns the linkified markup. */
function attributeNames(html: string): string[] {
  const host = document.createElement('div');
  host.innerHTML = html;
  return Array.from(host.querySelectorAll('*')).flatMap((el) =>
    Array.from(el.attributes).map((attr) => attr.name),
  );
}

function linkify(html: string): string {
  return linkifyPaths(html, [], [], { cache: false });
}

describe('linkifyPaths over an attribute value carrying angle brackets', () => {
  it('never turns a URL inside an attribute value into attribute names', () => {
    const rendered = renderMarkdown(
      '<p title="q>https://e.com/onmouseover=document.title=1;//">hello</p>',
    );
    const names = attributeNames(linkify(rendered));
    expect(names.filter((name) => name.startsWith('on'))).toEqual([]);
    expect(names).toEqual(['title']);
  });

  it('keeps the attribute value the author wrote', () => {
    const out = linkify('<p title="a > b &lt; c">x</p>');
    const host = document.createElement('div');
    host.innerHTML = out;
    expect(host.querySelector('p')?.getAttribute('title')).toBe('a > b < c');
  });

  it('still linkifies the text around such an element', () => {
    const out = linkify('<p><img alt="1 > 0" src="x.png"> see https://example.com/page</p>');
    const host = document.createElement('div');
    host.innerHTML = out;
    expect(host.querySelector('a')?.getAttribute('href')).toBe('https://example.com/page');
    expect(host.querySelector('img')?.getAttribute('alt')).toBe('1 > 0');
  });

  it('still rewrites an anchor whose title carries a bracket', () => {
    const out = linkify('<p><a title="x > y" href="https://example.com/">go</a> and https://e.com/</p>');
    const names = attributeNames(out);
    expect(names.filter((name) => name.startsWith('on'))).toEqual([]);
    const host = document.createElement('div');
    host.innerHTML = out;
    expect(host.querySelectorAll('a')).toHaveLength(2);
    expect(host.querySelector('a')?.getAttribute('title')).toBe('x > y');
  });

  it('leaves markup with no bracket in any attribute byte-for-byte alone', () => {
    const html = '<p title="plain">no links here</p>';
    expect(linkify(html)).toBe(html);
  });
});
