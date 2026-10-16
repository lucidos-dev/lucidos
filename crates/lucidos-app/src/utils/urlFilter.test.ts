import { describe, it, expect } from 'vitest';
import { isMainFrameUrl } from './urlFilter';

describe('isMainFrameUrl', () => {
  // Valid top-level URLs should pass
  it('accepts normal http URLs', () => {
    expect(isMainFrameUrl('https://vg.no')).toBe(true);
    expect(isMainFrameUrl('https://www.dn.no/article/123')).toBe(true);
    expect(isMainFrameUrl('http://example.com')).toBe(true);
    expect(isMainFrameUrl('https://en.wikipedia.org/wiki/Main_Page')).toBe(true);
  });

  // about: URLs should be rejected
  it('rejects about: URLs', () => {
    expect(isMainFrameUrl('about:')).toBe(false);
    expect(isMainFrameUrl('about:blank')).toBe(false);
    expect(isMainFrameUrl('about:srcdoc')).toBe(false);
  });

  // data: and blob: URLs should be rejected (iframe content)
  it('rejects data: and blob: URLs', () => {
    expect(isMainFrameUrl('data:text/html,<h1>test</h1>')).toBe(false);
    expect(isMainFrameUrl('blob:https://example.com/abc-123')).toBe(false);
  });

  // Ad tracker URLs should be rejected
  it('rejects Google ad tracking URLs', () => {
    expect(isMainFrameUrl('https://ep2.adtrafficquality.google/sodar/sodar2/253/runner.html')).toBe(false);
    expect(isMainFrameUrl('https://pagead2.googlesyndication.com/pagead/js/adsbygoogle.js')).toBe(false);
    expect(isMainFrameUrl('https://tpc.googlesyndication.com/safeframe/1-0-40/html/container.html')).toBe(false);
    expect(isMainFrameUrl('https://googleads.g.doubleclick.net/pagead/ads')).toBe(false);
    expect(isMainFrameUrl('https://securepubads.g.doubleclick.net/gampad/ads')).toBe(false);
  });

  it('rejects other common ad/tracker domains', () => {
    expect(isMainFrameUrl('https://ad.doubleclick.net/something')).toBe(false);
    expect(isMainFrameUrl('https://cdn.ampproject.org/v0.js')).toBe(false);
    expect(isMainFrameUrl('https://static.adsafeprotected.com/script.js')).toBe(false);
  });

  it('rejects safeframe and ad CDN URLs', () => {
    expect(isMainFrameUrl('https://adsdkprod.azureedge.net/assets/sf/v1.0.0-1/safeframe-v2.html')).toBe(false);
    expect(isMainFrameUrl('https://cdn.something.com/safeframe/container.html')).toBe(false);
    expect(isMainFrameUrl('https://ads.example.com/ad-iframe.html')).toBe(false);
  });

  // Empty and invalid URLs
  it('rejects empty and invalid URLs', () => {
    expect(isMainFrameUrl('')).toBe(false);
    expect(isMainFrameUrl('javascript:void(0)')).toBe(false);
  });

  // Should accept URLs even if they have query params or fragments
  it('accepts URLs with query params and fragments', () => {
    expect(isMainFrameUrl('https://vg.no/nyheter?id=123#top')).toBe(true);
    expect(isMainFrameUrl('https://www.google.com/search?q=test')).toBe(true);
  });

  // Google search is fine, Google ads are not
  it('distinguishes Google search from Google ads', () => {
    expect(isMainFrameUrl('https://www.google.com/search?q=test')).toBe(true);
    expect(isMainFrameUrl('https://www.google.com')).toBe(true);
    expect(isMainFrameUrl('https://pagead2.googlesyndication.com/anything')).toBe(false);
  });
});
