import { describe, it, expect } from 'vitest';
import { normalizeBasePath, baseContextValidFor, computeGatewayPickerHref } from './basePath';

// `normalizeBasePath` turns the server-stamped `<base href>` into a path prefix
// (no trailing slash, '' at root). It is slug-agnostic — any workspace name
// works, and `/~/` marks the picker context (ADR 0014).

describe('normalizeBasePath', () => {
  it('returns the /<slug> prefix for a workspace base href', () => {
    expect(normalizeBasePath('/dev/')).toBe('/dev');
    expect(normalizeBasePath('/work-2/')).toBe('/work-2');
    expect(normalizeBasePath('/a1b2/')).toBe('/a1b2');
  });

  it('returns /~ for the picker context', () => {
    expect(normalizeBasePath('/~/')).toBe('/~');
  });

  it('returns empty at a legacy root', () => {
    expect(normalizeBasePath('/')).toBe('');
  });

  it('strips an absolute-URL base href down to its pathname', () => {
    expect(normalizeBasePath('https://host.ts.net/dev/')).toBe('/dev');
  });

  it('tolerates a missing leading slash and extra trailing slashes', () => {
    expect(normalizeBasePath('dev/')).toBe('/dev');
    expect(normalizeBasePath('/dev///')).toBe('/dev');
  });
});

// `baseContextValidFor` is the defensive guard `main.tsx` uses to bounce a
// malformed workspace base-path context to the picker instead of rendering a
// broken app (the "string did not match the expected pattern" dead-end).
describe('baseContextValidFor', () => {
  it('treats the picker and legacy-root contexts (null slug) as valid', () => {
    expect(baseContextValidFor(null)).toBe(true);
  });

  it('accepts a well-formed workspace slug', () => {
    expect(baseContextValidFor('dev')).toBe(true);
    expect(baseContextValidFor('work-2')).toBe(true);
    expect(baseContextValidFor('a1b2')).toBe(true);
  });

  it('rejects a malformed slug the gateway would never mint', () => {
    expect(baseContextValidFor('My Workspace')).toBe(false);
    expect(baseContextValidFor('-bad')).toBe(false);
    expect(baseContextValidFor('weird~slug')).toBe(false);
    expect(baseContextValidFor('a/b')).toBe(false);
  });
});

// `computeGatewayPickerHref` builds the "Manage workspaces" link target. The bug
// it fixes: on a page served on the engine's OWN port the gateway picker lives at
// a *different* origin (its own port), so the origin-relative `/~/?pick` dead-ends
// (ADR 0014).
describe('computeGatewayPickerHref', () => {
  it('uses the relative picker route behind the gateway (same origin)', () => {
    expect(
      computeGatewayPickerHref({
        behindGateway: true,
        gatewayPort: 5251,
        protocol: 'https:',
        hostname: 'localhost',
      }),
    ).toBe('/~/?pick');
    // Relative even when no port is stamped — same origin is the gateway.
    expect(
      computeGatewayPickerHref({
        behindGateway: true,
        gatewayPort: null,
        protocol: 'https:',
        hostname: 'localhost',
      }),
    ).toBe('/~/?pick');
  });

  it('builds an absolute URL to the gateway on a direct engine-port page', () => {
    expect(
      computeGatewayPickerHref({
        behindGateway: false,
        gatewayPort: 5251,
        protocol: 'https:',
        hostname: 'localhost',
      }),
    ).toBe('https://localhost:5251/~/?pick');
  });

  it('targets the host the user is already on (Tailscale, http)', () => {
    expect(
      computeGatewayPickerHref({
        behindGateway: false,
        gatewayPort: 5251,
        protocol: 'http:',
        hostname: 'my-box.tail-scale.ts.net',
      }),
    ).toBe('http://my-box.tail-scale.ts.net:5251/~/?pick');
  });

  it('returns null on a direct page with no gateway (legacy no-gateway engine)', () => {
    expect(
      computeGatewayPickerHref({
        behindGateway: false,
        gatewayPort: null,
        protocol: 'https:',
        hostname: 'localhost',
      }),
    ).toBeNull();
  });
});
