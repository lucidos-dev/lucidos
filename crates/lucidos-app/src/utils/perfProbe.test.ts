import { describe, it, expect, beforeEach, afterEach, vi } from 'vitest';
// @ts-expect-error: Node APIs available at runtime via Vitest, no @types/node in project
import { readFileSync } from 'node:fs';
// @ts-expect-error: same
import { dirname, resolve } from 'node:path';
// @ts-expect-error: same
import { fileURLToPath } from 'node:url';
import {
  interactionSampleOf,
  startPerfProbe,
  _resetPerfProbeForTesting,
} from './perfProbe';
import {
  flushPerfQueue,
  _resetPerfQueueForTesting,
  _setPerfEnabledForTesting,
} from './perfQueue';

describe('interactionSampleOf: the split that IS the diagnosis', () => {
  const entry = {
    name: 'click',
    startTime: 1_000,
    duration: 150,
    processingStart: 1_080,
    processingEnd: 1_090,
  };

  it('splits the entry into input, handler and render', () => {
    expect(interactionSampleOf(entry)).toMatchObject({
      name: 'click',
      durationMs: 150,
      inputMs: 80,
      handlerMs: 10,
      renderMs: 60,
    });
  });

  it('describes no target as such rather than throwing on one', () => {
    expect(interactionSampleOf(entry).target).toBe('(no target)');
  });

  function element(f: {
    tag: string; id?: string; className?: string; role?: string; text?: string;
  }): Node {
    return {
      nodeType: 1,
      tagName: f.tag.toUpperCase(),
      id: f.id ?? '',
      className: f.className ?? '',
      textContent: f.text ?? '',
      getAttribute: (name: string) => (name === 'data-role' ? f.role ?? null : null),
    } as unknown as Node;
  }

  it('carries the target SHAPE and never its text', () => {
    const sample = interactionSampleOf({
      ...entry,
      target: element({
        tag: 'button',
        className: 'icon-btn send-btn',
        role: 'send',
        text: 'Reply to the surgery about the referral',
      }),
    });
    expect(sample.target).toBe('button.icon-btn.send-btn[data-role=send]');
    expect(sample.target).not.toContain('surgery');
    expect(sample.target).not.toContain('referral');
  });

  it('keeps the shape readable when the element has no class or role', () => {
    const target = element({ tag: 'div', text: 'a private thread title' });
    expect(interactionSampleOf({ ...entry, target }).target).toBe('div');
  });

  it('drops the element id, which is data-derived wherever the app sets one', () => {
    const target = element({
      tag: 'button',
      id: 'nav-1f3c9a02-7b41-4e55-9d2a-6c0e8b7f1d34',
      className: 'list-row',
      role: 'thread-row',
    });
    const shape = interactionSampleOf({ ...entry, target }).target;
    expect(shape).toBe('button.list-row[data-role=thread-row]');
    expect(shape).not.toContain('1f3c9a02');
  });

  it('rounds, so a fractional timestamp cannot bloat the log line', () => {
    const sample = interactionSampleOf({ ...entry, duration: 150.6, processingStart: 1_080.4 });
    expect(sample.durationMs).toBe(151);
    expect(sample.inputMs).toBe(80);
  });
});

describe('startPerfProbe: what it reports about itself', () => {
  let fetchMock: ReturnType<typeof vi.fn>;

  beforeEach(() => {
    _resetPerfProbeForTesting();
    _resetPerfQueueForTesting();
    fetchMock = vi.fn(() => Promise.resolve({ ok: true } as Response));
    vi.stubGlobal('fetch', fetchMock);
    vi.spyOn(console, 'warn').mockImplementation(() => {});
  });

  afterEach(() => {
    _setPerfEnabledForTesting(null);
    _resetPerfProbeForTesting();
    _resetPerfQueueForTesting();
    vi.unstubAllGlobals();
    vi.restoreAllMocks();
  });

  function stubObserver(accepts: string[]): void {
    class FakeObserver {
      observe(init: { type: string }): void {
        if (!accepts.includes(init.type)) throw new TypeError(`unsupported: ${init.type}`);
      }

      disconnect(): void {  }
    }
    vi.stubGlobal('PerformanceObserver', Object.assign(FakeObserver, {
      supportedEntryTypes: accepts,
    }));
  }

  function samples(): Array<Record<string, unknown>> {
    flushPerfQueue();
    return fetchMock.mock.calls.flatMap((call) => JSON.parse((call[1] as { body: string }).body));
  }

  it('records the observers a WebKit-shaped browser actually took', () => {
    _setPerfEnabledForTesting(true);
    stubObserver(['event']);
    startPerfProbe();
    const support = samples().find((s) => s.message === 'perf-probe-support');
    expect(support).toBeDefined();
    expect((support!.data as { armed: string[] }).armed).toEqual(['event']);
  });

  it('records an EMPTY armed list rather than staying silent', () => {
    _setPerfEnabledForTesting(true);
    stubObserver([]);
    startPerfProbe();
    const support = samples().find((s) => s.message === 'perf-probe-support');
    expect((support!.data as { armed: string[] }).armed).toEqual([]);
  });

  it('lists every observer a Chrome-shaped browser took', () => {
    _setPerfEnabledForTesting(true);
    stubObserver(['event', 'long-animation-frame', 'longtask']);
    startPerfProbe();
    const support = samples().find((s) => s.message === 'perf-probe-support');
    expect((support!.data as { armed: string[] }).armed)
      .toEqual(['event', 'long-animation-frame', 'longtask']);
  });

  it('still writes its console banner, which is the desktop half', () => {
    _setPerfEnabledForTesting(true);
    stubObserver(['event']);
    startPerfProbe();
    expect(console.warn).toHaveBeenCalledWith(expect.stringContaining('[perf-probe] active'));
  });

  it('posts nothing at all while recording is off', () => {
    _setPerfEnabledForTesting(false);
    stubObserver(['event', 'longtask']);
    startPerfProbe();
    flushPerfQueue();
    expect(fetchMock).not.toHaveBeenCalled();
  });

  it('records support when the toggle is flipped on mid-session', () => {
    _setPerfEnabledForTesting(false);
    stubObserver(['event']);
    startPerfProbe();
    expect(samples()).toEqual([]);

    _setPerfEnabledForTesting(true);
    const support = samples().find((s) => s.message === 'perf-probe-support');
    expect(support).toBeDefined();
    expect((support!.data as { armed: string[] }).armed).toEqual(['event']);
  });

  it('registers once, so a second call cannot double the observers', () => {
    _setPerfEnabledForTesting(true);
    stubObserver(['event']);
    startPerfProbe();
    startPerfProbe();
    expect(samples().filter((s) => s.message === 'perf-probe-support')).toHaveLength(1);
  });
});

describe('the probe cannot regress to console-only', () => {
  const here: string = dirname(fileURLToPath(import.meta.url));
  const source = readFileSync(resolve(here, 'perfProbe.ts'), 'utf-8');

  it('routes through the queue that reaches engine.log', () => {
    expect(source).toMatch(/import \{[^}]*\brecordPerfSample\b[^}]*\} from '\.\/perfQueue'/);
  });

  it('records a sample for each of the three observers, and for its support', () => {
    for (const name of ['interaction', 'loaf', 'longtask', 'perf-probe-support']) {
      expect(source).toContain(`recordPerfSample('${name}'`);
    }
  });

  it('builds the uploaded target from the structure reader, not the text one', () => {
    const built = source.slice(source.indexOf('export function interactionSampleOf'));
    const body = built.slice(built.indexOf('return {'), built.indexOf('\n    };'));
    expect(body).toContain('target: targetStructure(');
    expect(body).not.toContain('describeTarget(');
  });
});
