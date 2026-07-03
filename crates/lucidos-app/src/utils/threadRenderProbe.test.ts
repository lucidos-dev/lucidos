import { describe, it, expect, vi, beforeEach } from 'vitest';

// platform.ts touches navigator/window at import time (no jsdom here) — mock it.
let iosPwa = true;
vi.mock('./platform', () => ({ isIOSPwa: () => iosPwa }));
// liveness.ts touches the network / DOM at import time — stub postClientLog so we
// can assert what the probe would emit without a real fetch.
const postClientLog = vi.fn();
vi.mock('./liveness', () => ({ postClientLog: (...a: unknown[]) => postClientLog(...a) }));

import {
  classifyThreadRender,
  shouldReportThreadRender,
  reportThreadRenderProbe,
  type ThreadRenderProbe,
} from './threadRenderProbe';

/** A baseline "healthy content thread" probe; tests override the suspect fields. */
function probe(overrides: Partial<ThreadRenderProbe> = {}): ThreadRenderProbe {
  return {
    threadId: 't1',
    channel: 'chat',
    isCodingAgent: false,
    renderedExchangesLen: 3,
    freshExchangesLen: 3,
    eventCount: 42,
    eventsLoaded: true,
    hasContentEvents: true,
    animating: false,
    contentChildCount: 5,
    contentScrollHeight: 1200,
    ...overrides,
  };
}

describe('classifyThreadRender', () => {
  it('genuinely-empty when the thread has no content events (legit empty / draft)', () => {
    expect(classifyThreadRender(probe({ hasContentEvents: false, renderedExchangesLen: 0, freshExchangesLen: 0 })))
      .toBe('genuinely-empty');
  });

  it('missed-rerender: store HAS exchanges but the last render produced none', () => {
    // The "summary present but body empty on cold open, recovers on scroll" bug:
    // loadThreadEvents committed events to the store (freshExchangesLen > 0) but
    // ThreadView never re-rendered to show them (renderedExchangesLen === 0).
    expect(classifyThreadRender(probe({ renderedExchangesLen: 0, freshExchangesLen: 4, contentChildCount: 0 })))
      .toBe('missed-rerender');
  });

  it('empty-render: content events exist but a fresh fold still yields zero exchanges', () => {
    // Corrupted Map / grouping gap — the rebuildCorruptedThreadEvents domain.
    expect(classifyThreadRender(probe({ renderedExchangesLen: 0, freshExchangesLen: 0 })))
      .toBe('empty-render');
  });

  it('dom-missing: exchanges rendered but the content area has no DOM children', () => {
    expect(classifyThreadRender(probe({ renderedExchangesLen: 3, contentChildCount: 0 })))
      .toBe('dom-missing');
  });

  it('content-present: exchanges rendered and DOM built (healthy OR compositor paint loss)', () => {
    // JS cannot distinguish a healthy paint from an iOS layer whose texture was
    // recycled — a blank-body report landing here IS the paint-loss case.
    expect(classifyThreadRender(probe())).toBe('content-present');
  });

  it('null DOM metrics do not false-positive as dom-missing', () => {
    // ref not mounted yet → unknown DOM; must fold into content-present, not
    // dom-missing (which requires an explicit zero child count).
    expect(classifyThreadRender(probe({ contentChildCount: null, contentScrollHeight: null })))
      .toBe('content-present');
  });

  it('missed-rerender takes priority over empty-render when fresh exchanges exist', () => {
    expect(classifyThreadRender(probe({ renderedExchangesLen: 0, freshExchangesLen: 1 })))
      .toBe('missed-rerender');
  });

  // --- coding-agent variant (the latest repro: a CC thread with a spark icon) ---
  it('coding-agent thread: missed-rerender classified the same as chat', () => {
    expect(classifyThreadRender(probe({
      channel: 'claude_code', isCodingAgent: true,
      renderedExchangesLen: 0, freshExchangesLen: 1, contentChildCount: 0,
    }))).toBe('missed-rerender');
  });

  it('coding-agent thread: content-present (paint-loss suspect) classified the same as chat', () => {
    expect(classifyThreadRender(probe({ channel: 'claude_code', isCodingAgent: true })))
      .toBe('content-present');
  });
});

describe('shouldReportThreadRender', () => {
  it('skips the genuinely-empty class, reports every suspect class', () => {
    expect(shouldReportThreadRender('genuinely-empty')).toBe(false);
    for (const cls of ['missed-rerender', 'empty-render', 'dom-missing', 'content-present'] as const) {
      expect(shouldReportThreadRender(cls)).toBe(true);
    }
  });
});

describe('reportThreadRenderProbe', () => {
  beforeEach(() => { postClientLog.mockClear(); iosPwa = true; });

  it('emits a [render] breadcrumb with the class + raw fields for a suspect thread', () => {
    reportThreadRenderProbe(probe({ renderedExchangesLen: 0, freshExchangesLen: 4 }));
    expect(postClientLog).toHaveBeenCalledTimes(1);
    const [category, message, data] = postClientLog.mock.calls[0];
    expect(category).toBe('render');
    expect(message).toBe('thread_render_probe');
    expect(data).toMatchObject({ class: 'missed-rerender', fresh_exchanges: 4, rendered_exchanges: 0 });
  });

  it('does not emit for a genuinely-empty thread', () => {
    reportThreadRenderProbe(probe({ hasContentEvents: false, renderedExchangesLen: 0, freshExchangesLen: 0 }));
    expect(postClientLog).not.toHaveBeenCalled();
  });

  it('does not emit off iOS PWA (keeps desktop / dev logs quiet)', () => {
    iosPwa = false;
    reportThreadRenderProbe(probe({ renderedExchangesLen: 0, freshExchangesLen: 4 }));
    expect(postClientLog).not.toHaveBeenCalled();
  });

  it('emits for a healthy-looking content-present thread (the paint-loss population)', () => {
    reportThreadRenderProbe(probe());
    expect(postClientLog).toHaveBeenCalledTimes(1);
    expect(postClientLog.mock.calls[0][2]).toMatchObject({ class: 'content-present' });
  });
});
