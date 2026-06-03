import { describe, it, expect, beforeEach, afterEach } from 'vitest';

import { scrollToEventAndPulse } from '../scrollState';

type MOCallback = (records: MutationRecord[], observer: MutationObserver) => void;
const moObservations: Array<{ target: any; options: any }> = [];
let lastMoCallback: MOCallback | null = null;

class FakeMutationObserver {
  constructor(cb: MOCallback) { lastMoCallback = cb; }
  observe(target: any, options: any) { moObservations.push({ target, options }); }
  disconnect() {}
  takeRecords() { return []; }
}

const fakeBody = { tagName: 'BODY' };

function installFakeDom(opts: { threadContents?: any[]; dataEventMatches?: any[] } = {}) {
  const orig = {
    documentBody: (globalThis.document as any).body,
    documentQSA: (globalThis.document as any).querySelectorAll,
    MutationObserver: (globalThis as any).MutationObserver,
    CSS: (globalThis as any).CSS,
  };

  (globalThis.document as any).body = fakeBody;
  (globalThis.document as any).querySelectorAll = (sel: string) => {
    if (sel === '.thread-content') return opts.threadContents ?? [];
    if (sel.startsWith('[data-event-id')) return opts.dataEventMatches ?? [];
    return [];
  };
  (globalThis as any).MutationObserver = FakeMutationObserver;
  (globalThis as any).CSS = { escape: (s: string) => s };

  return () => {
    (globalThis.document as any).body = orig.documentBody;
    (globalThis.document as any).querySelectorAll = orig.documentQSA;
    (globalThis as any).MutationObserver = orig.MutationObserver;
    (globalThis as any).CSS = orig.CSS;
  };
}

beforeEach(() => {
  moObservations.length = 0;
  lastMoCallback = null;
});

describe('scrollToEventAndPulse — MutationObserver setup', () => {
  let restore: (() => void) | null = null;
  afterEach(() => { restore?.(); restore = null; });

  it('observes document.body, not the loading-state .thread-content (which gets detached on ThreadView loading→loaded swap)', () => {
    // ThreadView's loading branch and loaded branch are structurally different
    // children of .thread-view (1 vs 2), so Preact's positional diff cannot
    // preserve the loading-branch .thread-content across the swap. An observer
    // scoped to it would strand. iOS PWA cold-start hits this every time.
    const loadingThreadContent = { tagName: 'DIV', className: 'thread-content' };
    restore = installFakeDom({ threadContents: [loadingThreadContent] });

    scrollToEventAndPulse('e-7');

    expect(moObservations).toHaveLength(1);
    expect(moObservations[0].target).toBe(fakeBody);
    expect(moObservations[0].options).toMatchObject({ childList: true, subtree: true });
  });

  it('observes document.body when no .thread-content exists yet', () => {
    restore = installFakeDom({});
    scrollToEventAndPulse('e-7');
    expect(moObservations).toHaveLength(1);
    expect(moObservations[0].target).toBe(fakeBody);
  });

  it('is a no-op when called with an empty event id', () => {
    restore = installFakeDom({});
    scrollToEventAndPulse('');
    expect(moObservations).toHaveLength(0);
  });
});

describe('scrollToEventAndPulse — MutationObserver callback filter', () => {
  let restore: (() => void) | null = null;
  afterEach(() => { restore?.(); restore = null; });

  function makeNode(opts: {
    nodeType?: number;
    matchesSelector?: boolean;
    querySelectorResult?: any;
  }): Node & Element {
    let qsCalls = 0;
    let matchCalls = 0;
    const node: any = {
      nodeType: opts.nodeType ?? 1,
      matches: (_sel: string) => { matchCalls++; return !!opts.matchesSelector; },
      querySelector: (_sel: string) => { qsCalls++; return opts.querySelectorResult ?? null; },
      get _matchCalls() { return matchCalls; },
      get _qsCalls() { return qsCalls; },
    };
    return node;
  }

  function fireMutation(addedNodes: Node[]) {
    const records = [{ addedNodes, type: 'childList' } as unknown as MutationRecord];
    lastMoCallback?.(records, {} as MutationObserver);
  }

  it('does NOT re-query document when added nodes do not contain the target (streaming-token wakeup case)', () => {
    restore = installFakeDom({});
    let docQsaCalls = 0;
    const origQsa = (globalThis.document as any).querySelectorAll;
    (globalThis.document as any).querySelectorAll = (sel: string) => {
      docQsaCalls++;
      return origQsa(sel);
    };

    scrollToEventAndPulse('e-7');
    const docQsaCallsAfterSetup = docQsaCalls;

    const streamingTextNode = makeNode({ nodeType: 3 });
    const unrelatedElement = makeNode({ matchesSelector: false, querySelectorResult: null });
    fireMutation([streamingTextNode, unrelatedElement]);

    // No call into document.querySelectorAll past the initial setup.
    expect(docQsaCalls).toBe(docQsaCallsAfterSetup);
  });

  it('re-queries when an added node IS the target', () => {
    restore = installFakeDom({});
    scrollToEventAndPulse('e-7');

    let queriedSelector: string | null = null;
    (globalThis.document as any).querySelectorAll = (sel: string) => {
      queriedSelector = sel;
      return [];
    };

    fireMutation([makeNode({ matchesSelector: true })]);

    expect(queriedSelector).toBe('[data-event-id="e-7"]');
  });

  it('re-queries when an added subtree CONTAINS the target', () => {
    restore = installFakeDom({});
    scrollToEventAndPulse('e-7');

    let queriedSelector: string | null = null;
    (globalThis.document as any).querySelectorAll = (sel: string) => {
      queriedSelector = sel;
      return [];
    };

    fireMutation([makeNode({ matchesSelector: false, querySelectorResult: {} })]);

    expect(queriedSelector).toBe('[data-event-id="e-7"]');
  });
});
