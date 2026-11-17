// @vitest-environment jsdom
import { describe, expect, it } from 'vitest';
import { scopeRemoteContent } from './remoteContent';

describe('remote content in the trusted document', () => {
  function scrub(html: string): Element {
    const template = document.createElement('template');
    template.innerHTML = html;
    const node = template.content.firstElementChild!;
    scopeRemoteContent(node, '/demo@remote', 'https://gateway.test');
    return node;
  }

  it('refuses automatic requests to private or sibling routes', () => {
    for (const src of ['/~/api/v1/control/workspaces', '/other@sibling/api/v1/threads', '../~/api/v1/control/workspaces']) {
      expect(scrub(`<img src="${src}">`).hasAttribute('src'), src).toBe(false);
    }
    expect(scrub('<svg href="/~/api/v1/control/workspaces"></svg>').hasAttribute('href')).toBe(false);
    expect(scrub('<svg filter="url(/private#filter)"></svg>').hasAttribute('filter')).toBe(false);
  });

  it('keeps scoped images and external images', () => {
    for (const src of ['/demo@remote/api/v1/data/cover.png', '/demo@remote/data/cover.png', 'https://images.example.test/cover.png']) {
      expect(scrub(`<img src="${src}">`).getAttribute('src')).toBe(src);
    }
  });

  it('keeps presentation while removing CSS resource and srcset channels', () => {
    expect(scrub('<span style="color:red">text</span>').getAttribute('style')).toBe('color:red');
    for (const style of ['background:url(/~/api/v1/control/workspaces)', 'background:u\\72l(/private)', 'background:image-set("/private" 1x)', 'color:red;/*x*/background:url(/private)']) {
      const node = document.createElement('span');
      node.setAttribute('style', style);
      scopeRemoteContent(node, '/demo@remote', 'https://gateway.test');
      expect(node.hasAttribute('style'), style).toBe(false);
    }
    expect(scrub('<img srcset="/private 2x">').hasAttribute('srcset')).toBe(false);
  });
});
