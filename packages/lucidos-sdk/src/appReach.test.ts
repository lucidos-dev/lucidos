/**
 * The matcher over the engine's generated answer.
 *
 * The table itself is the engine's, and its own tests cover what is in it.
 * These cases are about the matching: a param must not swallow a segment, a
 * wildcard must not reach a sibling route, and an unknown route is refused.
 */
import { describe, it, expect } from 'vitest';
import { appMayCall, normalizeSuffix, pathMatchesPattern } from './appReach';

describe('pathMatchesPattern', () => {
  it('matches a literal route exactly', () => {
    expect(pathMatchesPattern('/threads/list', '/threads/list')).toBe(true);
    expect(pathMatchesPattern('/threads/list', '/threads/listing')).toBe(false);
    expect(pathMatchesPattern('/threads/list', '/threads')).toBe(false);
  });

  it('takes exactly one segment for a param', () => {
    const p = '/oauth/:provider/access-token';
    expect(pathMatchesPattern(p, '/oauth/spotify/access-token')).toBe(true);
    expect(pathMatchesPattern(p, '/oauth/a/b/access-token')).toBe(false);
    expect(pathMatchesPattern(p, '/oauth//access-token')).toBe(false);
  });

  it('takes the remainder for a wildcard, and needs one segment', () => {
    expect(pathMatchesPattern('/data/*path', '/data/artifacts/x.md')).toBe(true);
    expect(pathMatchesPattern('/data/*path', '/data/x')).toBe(true);
    expect(pathMatchesPattern('/data/*path', '/data/')).toBe(false);
    expect(pathMatchesPattern('/data/*path', '/data')).toBe(false);
  });

  it('keeps a trailing slash significant', () => {
    expect(pathMatchesPattern('/proxy/:name/', '/proxy/sonos/')).toBe(true);
    expect(pathMatchesPattern('/proxy/:name', '/proxy/sonos/')).toBe(false);
  });
});

describe('appMayCall', () => {
  it('admits what a shipped namespace calls', () => {
    expect(appMayCall('GET', '/data/artifacts/x.md')).toBe(true);
    expect(appMayCall('PUT', '/data/artifacts/x.md')).toBe(true);
    expect(appMayCall('GET', '/threads/list')).toBe(true);
    expect(appMayCall('POST', '/ui/navigate')).toBe(true);
    expect(appMayCall('POST', '/proxy/sonos/living-room/play')).toBe(true);
  });

  it('admits the three routes the hatch opened', () => {
    expect(appMayCall('GET', '/env-vars')).toBe(true);
    expect(appMayCall('GET', '/models')).toBe(true);
    expect(appMayCall('POST', '/notifications')).toBe(true);
  });

  // A user env var lands in every run_bash and run_python, so a name like
  // PYTHONPATH is host code execution. The engine keeps the read and refuses
  // the writes; this is the matcher half of that answer.
  it('refuses a write to the env vars the agent runs under', () => {
    expect(appMayCall('POST', '/env-vars')).toBe(false);
    expect(appMayCall('PUT', '/env-vars')).toBe(false);
    expect(appMayCall('DELETE', '/env-vars')).toBe(false);
  });

  it('refuses a route the engine classified away from apps', () => {
    expect(appMayCall('GET', '/credential-value')).toBe(false);
    expect(appMayCall('POST', '/chat/stream')).toBe(false);
    expect(appMayCall('POST', '/threads/abc/answer-question')).toBe(false);
    expect(appMayCall('GET', '/messages')).toBe(false);
    expect(appMayCall('GET', '/app/notes/source')).toBe(false);
  });

  it('refuses a method the route does not open', () => {
    expect(appMayCall('DELETE', '/models')).toBe(false);
    expect(appMayCall('DELETE', '/preferences')).toBe(false);
    expect(appMayCall('PUT', '/app')).toBe(false);
  });

  it('answers from the route the engine would pick, not from every match', () => {
    // `/data/edit` matches the `/data/*path` wildcard too. The literal route
    // is the one axum routes to, and it serves POST alone.
    expect(appMayCall('POST', '/data/edit')).toBe(true);
    expect(appMayCall('GET', '/data/edit')).toBe(false);
    expect(appMayCall('PUT', '/data/edit')).toBe(false);
    expect(appMayCall('DELETE', '/data/edit')).toBe(false);
    // Same for upload, and the wildcard still answers for anything else.
    expect(appMayCall('POST', '/data/upload')).toBe(true);
    expect(appMayCall('DELETE', '/data/upload')).toBe(false);
    expect(appMayCall('DELETE', '/data/artifacts/x.md')).toBe(true);
  });

  it('refuses a route nobody classified', () => {
    expect(appMayCall('GET', '/not-a-route')).toBe(false);
    // A plain prefix check would take this, and it is a different route.
    expect(appMayCall('GET', '/models-registry')).toBe(false);
    expect(appMayCall('GET', '/data-export')).toBe(false);
  });
});

describe('normalizeSuffix', () => {
  it('resolves a dot segment before anything is checked', () => {
    expect(normalizeSuffix('/data/../credentials')?.pathname).toBe('/credentials');
    expect(normalizeSuffix('/data/%2e%2e/credentials')?.pathname).toBe('/credentials');
    // So the traversal is refused on what it actually resolves to.
    expect(appMayCall('GET', normalizeSuffix('/data/%2e%2e/credentials')!.pathname)).toBe(false);
  });

  it('keeps a `..` that is part of a filename', () => {
    expect(normalizeSuffix('/data/artifacts/..hidden.md')?.pathname)
      .toBe('/data/artifacts/..hidden.md');
  });

  it('hands back the query and hash with the path', () => {
    expect(normalizeSuffix('/data/artifacts/x.md?v=2')?.full).toBe('/data/artifacts/x.md?v=2');
  });

  it('refuses a protocol-relative or unrooted path', () => {
    expect(normalizeSuffix('//example.com/data')).toBeNull();
    expect(normalizeSuffix('data/x')).toBeNull();
    expect(normalizeSuffix('')).toBeNull();
  });
});
