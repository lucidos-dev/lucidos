import { describe, expect, it } from 'vitest';
import { previewPathFromEnv, previewWorkspacePrefix, stampPreviewBase } from '../../vite/frontendPreviewPath';

describe('gateway preview paths', () => {
  it('activates only for the socket launcher, with a canonical route', () => {
    expect(previewPathFromEnv({})).toBeUndefined();
    expect(() => previewPathFromEnv({ LUCIDOS_FRONTEND_PREVIEW_SOCKET: '/socket' })).toThrow();
    expect(previewPathFromEnv({
      LUCIDOS_FRONTEND_PREVIEW_SOCKET: '/socket',
      LUCIDOS_FRONTEND_PREVIEW_WORKSPACE_ID: 'demo',
      LUCIDOS_FRONTEND_PREVIEW_KEY: '0123456789abcdef',
    })).toBe('/~preview/0123456789abcdef/demo/');
  });

  it('accepts local and relayed workspace prefixes for this workspace only', () => {
    for (const prefix of ['/demo/', '/demo@remote/']) {
      expect(previewWorkspacePrefix(prefix, 'demo')).toBe(prefix);
    }
  });

  it('rejects injection, other workspaces, malformed routes and multiple headers', () => {
    for (const prefix of [undefined, ['/demo/'], '/elsewhere/', '//demo/', '/demo//',
      '/demo@/', '/demo@remote/extra/', '/demo/" onload="alert(1)', 'https://example.com/demo/']) {
      expect(previewWorkspacePrefix(prefix, 'demo')).toBe('/demo/');
    }
  });

  it('replaces the data base without rewriting absolute module paths', () => {
    expect(stampPreviewBase('<head><base href="/"><script src="/~preview/key/demo/main.js"></script></head>', '/demo@remote/'))
      .toBe('<head><meta name="lucidos-frontend-preview" content="true"><base href="/demo@remote/"><script src="/~preview/key/demo/main.js"></script></head>');
  });
});
