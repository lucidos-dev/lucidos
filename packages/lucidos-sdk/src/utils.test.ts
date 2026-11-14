// @vitest-environment jsdom
//
// jsdom, not the `document` stub in test-setup.ts: the property under test is
// how a real HTML parser reads the escaped markup.
import { describe, it, expect } from 'vitest';
import { utils } from './utils';

/** Parses `markup` and returns the attributes its first element ended up with. */
function attributesOf(markup: string): Record<string, string> {
  const host = document.createElement('div');
  host.innerHTML = markup;
  const el = host.firstElementChild!;
  return Object.fromEntries(Array.from(el.attributes, (a) => [a.name, a.value]));
}

describe('utils.escapeHtmlAttr', () => {
  const hostile = `x" onmouseover="alert(1)' onfocus='alert(2)"><script>alert(3)</script>&`;

  it('keeps a hostile value inside a double-quoted attribute', () => {
    const attrs = attributesOf(`<a title="${utils.escapeHtmlAttr(hostile)}"></a>`);
    expect(attrs).toEqual({ title: hostile });
  });

  it('keeps a hostile value inside a single-quoted attribute', () => {
    const attrs = attributesOf(`<a title='${utils.escapeHtmlAttr(hostile)}'></a>`);
    expect(attrs).toEqual({ title: hostile });
  });

  it('escapes every markup-significant character', () => {
    expect(utils.escapeHtmlAttr(`&<>"'`)).toBe('&amp;&lt;&gt;&quot;&#39;');
  });

  it('round-trips through text position as the original string', () => {
    const host = document.createElement('div');
    host.innerHTML = utils.escapeHtmlAttr(hostile);
    expect(host.textContent).toBe(hostile);
    expect(host.children).toHaveLength(0);
  });
});

describe('utils.escapeHtmlAttr from plain JS', () => {
  const looseAttr = utils.escapeHtmlAttr as (value: unknown) => string;
  const looseText = utils.escapeHtml as (value: unknown) => string;

  it('coerces a non-string the way escapeHtml does', () => {
    for (const value of [42, null, undefined]) {
      expect(looseAttr(value)).toBe(looseText(value));
    }
  });
});

describe('utils.escapeHtml', () => {
  it('is text-position only: it leaves a quote raw', () => {
    expect(utils.escapeHtml(`<b>"'&`)).toBe(`&lt;b&gt;"'&amp;`);
  });
});
