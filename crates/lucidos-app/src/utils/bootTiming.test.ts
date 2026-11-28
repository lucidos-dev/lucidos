import { describe, it, expect, vi, beforeEach, afterEach } from 'vitest';

const recordPerfSample = vi.fn();
vi.mock('./perfQueue', () => ({ recordPerfSample: (...args: unknown[]) => recordPerfSample(...args) }));

import {
  LANDING_GRACE_MS,
  _resetBootTimingForTesting,
  bootDeepLinkLanded,
  markBoot,
  urlCarriesDeepLink,
} from './bootTiming';

let now = 0;

function boot(url: string) {
  const parsed = new URL(url);
  vi.stubGlobal('location', { search: parsed.search, hash: parsed.hash });
  markBoot('clientStarted');
}

beforeEach(() => {
  vi.useFakeTimers();
  now = 0;
  vi.spyOn(performance, 'now').mockImplementation(() => now);
  recordPerfSample.mockClear();
  _resetBootTimingForTesting();
});

afterEach(() => {
  vi.useRealTimers();
  vi.unstubAllGlobals();
  vi.restoreAllMocks();
});

describe('boot timing', () => {
  it('reads a notification deep link from the query or the hash, by value', () => {
    expect(urlCarriesDeepLink('?notification=n1&thread=t1', '')).toBe(true);
    expect(urlCarriesDeepLink('', '#notification=n1')).toBe(true);
    expect(urlCarriesDeepLink('?a=1', '#thread=t1&notification=n1')).toBe(true);
    expect(urlCarriesDeepLink('?notification=', '')).toBe(false);
    expect(urlCarriesDeepLink('?xnotification=n1', '')).toBe(false);
    expect(urlCarriesDeepLink('', '')).toBe(false);
  });

  it('honours routes and empty hash values that take precedence over a notification query', () => {
    expect(urlCarriesDeepLink('?notification=n1', '#thread=1e6a2f14-0000-4000-8000-000000000000')).toBe(false);
    expect(urlCarriesDeepLink('?notification=n1', '#notifications')).toBe(false);
    expect(urlCarriesDeepLink('?notification=n1', '#app=demo')).toBe(false);
    expect(urlCarriesDeepLink('?notification=n1', '#notification=')).toBe(false);
    expect(urlCarriesDeepLink('?notification=n1', '#notification=n2')).toBe(true);
  });

  it('sends one sample once a push tap has landed under a lifted splash', () => {
    boot('https://host/myws/?notification=n1&thread=t1');
    now = 410;
    markBoot('connected');
    now = 520;
    markBoot('threadsLoaded');
    now = 560;
    markBoot('splashLifted');
    expect(recordPerfSample).not.toHaveBeenCalled();
    now = 640;
    markBoot('landed');

    expect(recordPerfSample).toHaveBeenCalledTimes(1);
    const [name, sample] = recordPerfSample.mock.calls[0];
    expect(name).toBe('boot');
    expect(sample).toMatchObject({
      deepLink: true,
      clientStartedMs: 0,
      connectedMs: 410,
      threadsLoadedMs: 520,
      splashLiftedMs: 560,
      landedMs: 640,
    });
  });

  it('keeps the first time a milestone was reached', () => {
    boot('https://host/myws/?notification=n1');
    now = 100;
    markBoot('connected');
    now = 900;
    markBoot('connected');
    markBoot('splashLifted');
    markBoot('landed');
    vi.advanceTimersByTime(LANDING_GRACE_MS);
    expect(recordPerfSample.mock.calls[0][1].connectedMs).toBe(100);
  });

  it('sends without a landing once the grace runs out', () => {
    boot('https://host/myws/');
    markBoot('splashLifted');
    vi.advanceTimersByTime(LANDING_GRACE_MS - 1);
    expect(recordPerfSample).not.toHaveBeenCalled();
    vi.advanceTimersByTime(1);
    expect(recordPerfSample).toHaveBeenCalledTimes(1);
    expect(recordPerfSample.mock.calls[0][1].landedMs).toBeUndefined();
  });

  it('ignores a landing on a document that booted without a deep link', () => {
    boot('https://host/myws/');
    markBoot('splashLifted');
    markBoot('landed');
    expect(bootDeepLinkLanded.value).toBe(false);
    expect(recordPerfSample).not.toHaveBeenCalled();
  });

  it('signals a notification landing and still measures a later thread list', () => {
    boot('https://host/myws/?notification=n1');
    expect(bootDeepLinkLanded.value).toBe(false);
    now = 100;
    markBoot('landed');
    expect(bootDeepLinkLanded.value).toBe(true);
    markBoot('splashLifted');
    expect(recordPerfSample).not.toHaveBeenCalled();
    now = 200;
    markBoot('threadsLoaded');
    expect(recordPerfSample.mock.calls[0][1]).toMatchObject({
      landedMs: 100,
      splashLiftedMs: 100,
      threadsLoadedMs: 200,
    });
  });

  it('reports nothing for a document that never started a workspace client', () => {
    markBoot('splashLifted');
    vi.advanceTimersByTime(LANDING_GRACE_MS);
    expect(recordPerfSample).not.toHaveBeenCalled();
  });

  it('sends at most once', () => {
    boot('https://host/myws/?notification=n1');
    markBoot('splashLifted');
    markBoot('landed');
    vi.advanceTimersByTime(LANDING_GRACE_MS);
    markBoot('connected');
    expect(recordPerfSample).toHaveBeenCalledTimes(1);
  });
});
