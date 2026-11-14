import { describe, it, expect } from 'vitest';
import { bridgePathFor, faceFamily, fontFamilies, inlineCssUrls, type Inliner } from './captureResources';

const DOC = 'https://host.example/ws/app/habit-tracker/';

describe('bridgePathFor', () => {
  const path = (url: string, base = '/ws') => bridgePathFor(url, base, DOC);

  it('reads an engine route as its /api/v1 suffix, query kept', () => {
    expect(path('/ws/api/v1/fonts/fira-code-6.2.woff2')).toBe('/fonts/fira-code-6.2.woff2');
    expect(path('https://host.example/ws/api/v1/fonts/a.css?v=2')).toBe('/fonts/a.css?v=2');
  });

  it('reads the data mount through the data route', () => {
    expect(path('/ws/data/artifacts/chart.png')).toBe('/data/artifacts/chart.png');
  });

  it('reads an app file where the engine serves it from', () => {
    expect(path('logo.png')).toBe('/data/apps/habit-tracker/logo.png');
    expect(path('/ws/app/habit-tracker/img/a%20b.png')).toBe('/data/apps/habit-tracker/img/a%20b.png');
    expect(path('/ws/app/habit-tracker/artifacts/report.png')).toBe('/data/artifacts/report.png');
  });

  it('drops the frame capability pass, which the bridge does not need', () => {
    expect(path('/ws/~cap/tok123/data/fonts/brand/a.woff2')).toBe('/data/fonts/brand/a.woff2');
    expect(path('/ws/~cap/tok123/app/habit-tracker/logo.png')).toBe('/data/apps/habit-tracker/logo.png');
  });

  it('works direct to the engine, with no workspace prefix', () => {
    expect(bridgePathFor('/api/v1/fonts/a.css', '', 'http://127.0.0.1:5252/app/x/')).toBe('/fonts/a.css');
    expect(bridgePathFor('logo.png', '', 'http://127.0.0.1:5252/app/x/')).toBe('/data/apps/x/logo.png');
  });

  it('refuses what the bridge cannot reach', () => {
    // Another host.
    expect(path('https://cdn.example/x.png')).toBeNull();
    // Another workspace on the same gateway.
    expect(path('/other/data/x.png')).toBeNull();
    // A WIP preview reads a worktree, and the live copy would be the wrong file.
    expect(path('/ws/app/habit-tracker/logo.png?thread_id=t1')).toBeNull();
    // Not a URL at all.
    expect(path('http://[bad')).toBeNull();
  });
});

describe('inlineCssUrls', () => {
  const inline: Inliner = async (url) => (url.endsWith('missing.png') ? null : `data:x;base64,${btoa(url)}`);

  it('replaces each url() with its data URL, resolved against the base', async () => {
    const css = '.a{background-image:url("img/a.png")}.b{background:url(\'/b.png\') no-repeat}.c{mask:url(c.svg)}';
    const out = await inlineCssUrls(css, 'https://host.example/ws/app/x/', inline);
    expect(out).toContain(`url("data:x;base64,${btoa('https://host.example/ws/app/x/img/a.png')}")`);
    expect(out).toContain(`url("data:x;base64,${btoa('https://host.example/b.png')}")`);
    expect(out).toContain(`url("data:x;base64,${btoa('https://host.example/ws/app/x/c.svg')}")`);
  });

  it('leaves a data URL alone, parentheses and quotes inside included', async () => {
    const css = '.a{background:url("data:image/svg+xml,%3Csvg%3E(\\"x\\")%3C/svg%3E")}';
    expect(await inlineCssUrls(css, DOC, inline)).toBe(css);
  });

  it('empties a url() it cannot read, so it paints nothing', async () => {
    expect(await inlineCssUrls('.a{background:url(missing.png)}', DOC, inline)).toBe('.a{background:url("")}');
  });
});

describe('font family names', () => {
  it('lists a font-family value unquoted and lowercased', () => {
    expect(fontFamilies('"Fira Code", \'Brand Sans\', monospace')).toEqual(['fira code', 'brand sans', 'monospace']);
  });

  it('reads the family a @font-face declares', () => {
    expect(faceFamily('@font-face { font-family: "Fira Code"; src: url(a.woff2); }')).toBe('fira code');
    expect(faceFamily('@font-face{src:url(a.woff2)}')).toBeNull();
  });
});
