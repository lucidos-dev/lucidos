// @vitest-environment jsdom
import { describe, expect, it } from 'vitest';
import { mermaidFrameDocument } from './mermaidFrame';

describe('Mermaid frame boundary', () => {
  it('blocks resource loads and only permits our nonce-bearing scripts', () => {
    const html = mermaidFrameDocument('/* bundled library */', 'token');
    expect(html).toContain("default-src 'none'");
    expect(html).toContain("script-src 'nonce-token'");
    expect(html).not.toContain('allow-same-origin');
    expect(html).toContain('securityLevel: \'strict\'');
  });
  it('cannot terminate the library script with embedded HTML', () => {
    const html = mermaidFrameDocument('/* </script><img src=x> */', 'token');
    expect(html).not.toContain('</script><img');
    expect(html.match(/<script nonce=/g)).toHaveLength(2);
  });
});
