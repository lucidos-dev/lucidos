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
  // An interaction that waited 80ms before our handler even ran, spent 10ms in
  // it, and then 60ms reaching the screen. The input share says the main thread
  // was already busy, which points away from the handler entirely.
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

  /** An element as the structure reader sees it. Built by hand because
   *  `test-setup.ts` stubs `document` without real nodes, and this function is
   *  pure over exactly these five reads. */
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
    // The uploaded line persists in engine.log. An element's text is user
    // content: a thread title, a file name, the opening of a message. Only the
    // structure we author ourselves may travel.
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
    // A thread drawer row carries `navKeyDomId(thread.meta.id)`, so keeping the
    // id would put a thread id in engine.log on every tap of a row.
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

  /** A PerformanceObserver that accepts only the named entry types, the way a
   *  real engine refuses the ones it does not implement. WebKit is the case
   *  this whole support sample exists for. */
  function stubObserver(accepts: string[]): void {
    class FakeObserver {
      observe(init: { type: string }): void {
        if (!accepts.includes(init.type)) throw new TypeError(`unsupported: ${init.type}`);
      }

      disconnect(): void { /* nothing to tear down */ }
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
    // Safari: Event Timing at best, and neither of the two decisive ones.
    stubObserver(['event']);
    startPerfProbe();
    const support = samples().find((s) => s.message === 'perf-probe-support');
    expect(support).toBeDefined();
    expect((support!.data as { armed: string[] }).armed).toEqual(['event']);
  });

  it('records an EMPTY armed list rather than staying silent', () => {
    // The reading that cost a whole round: a log with no perf lines meant
    // either "nothing was slow" or "nothing could be seen", and nothing said
    // which. An empty list says which.
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
    // THE supported path, and the one a startup-only sample missed entirely.
    // The gate is off at boot for everyone, so the boot sample is dropped by
    // the queue. The user then switches recording on and reproduces.
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
  // The regression this whole change exists to undo: findings that only ever
  // reached `console.warn`, which an iOS PWA gives no way to read. A source
  // scan, because the value is in the WIRING and no behaviour test sees a
  // deleted call.
  const here: string = dirname(fileURLToPath(import.meta.url));
  const source = readFileSync(resolve(here, 'perfProbe.ts'), 'utf-8');

  it('routes through the queue that reaches engine.log', () => {
    // The import LIST is free to grow, so match the symbol and its source
    // rather than one spelling of the line.
    expect(source).toMatch(/import \{[^}]*\brecordPerfSample\b[^}]*\} from '\.\/perfQueue'/);
  });

  it('records a sample for each of the three observers, and for its support', () => {
    for (const name of ['interaction', 'loaf', 'longtask', 'perf-probe-support']) {
      expect(source).toContain(`recordPerfSample('${name}'`);
    }
  });

  it('builds the uploaded target from the structure reader, not the text one', () => {
    // The sample persists in engine.log, so it takes `targetStructure`.
    // `describeTarget` appends the element's own text and belongs to the
    // console line alone.
    // Anchored on the RETURN, not the signature: the parameter's inline type
    // is itself a brace block, so a naive end-of-function search stops inside it.
    const built = source.slice(source.indexOf('export function interactionSampleOf'));
    const body = built.slice(built.indexOf('return {'), built.indexOf('\n    };'));
    expect(body).toContain('target: targetStructure(');
    expect(body).not.toContain('describeTarget(');
  });
});
