import { describe, it, expect, beforeEach, vi } from 'vitest';
// @ts-expect-error: Node APIs available at runtime via Vitest, no @types/node in project
import { readFileSync } from 'node:fs';

import { srcPath } from '../../../__tests__/staticImportGraph';

const classifyScroll = vi.hoisted(() => vi.fn());
vi.mock('../scrollState', () => ({ classifyScroll }));

import { onClassifiedScroll, onClassifiedScrollAsEngine } from '../classifiedScroll';

describe('one classification per scroll event', () => {
  function makeContainer() {
    const scroll: Function[] = [];
    const el: any = {
      addEventListener: (type: string, fn: Function) => { if (type === 'scroll') scroll.push(fn); },
      removeEventListener: (type: string, fn: Function) => {
        if (type === 'scroll' && scroll.includes(fn)) scroll.splice(scroll.indexOf(fn), 1);
      },
      fireScroll: () => { for (const fn of [...scroll]) fn({ type: 'scroll' }); },
      domListeners: () => scroll.length,
    };
    return el;
  }

  beforeEach(() => {
    classifyScroll.mockReset();
    classifyScroll.mockImplementation(() => ({ navigation: false }));
  });

  it('classifies once for the engine, the memory, the bars and the indicator', () => {
    const el = makeContainer();
    const seen: Record<string, unknown[]> = { engine: [], memory: [], bars: [], indicator: [] };
    const stops = [
      onClassifiedScroll(el, (scroll) => seen.memory.push(scroll)),
      onClassifiedScroll(el, (scroll) => seen.bars.push(scroll)),
      onClassifiedScrollAsEngine(el, (scroll) => { seen.engine.push(scroll); return scroll; }),
      onClassifiedScroll(el, (scroll) => seen.indicator.push(scroll)),
    ];
    el.fireScroll();
    expect(classifyScroll).toHaveBeenCalledTimes(1);
    expect(classifyScroll).toHaveBeenCalledWith(el);
    const cls = classifyScroll.mock.results[0].value;
    for (const role of Object.keys(seen)) expect(seen[role]).toEqual([cls]);
    for (const role of Object.keys(seen)) expect(seen[role][0]).toBe(cls);
    el.fireScroll();
    expect(classifyScroll).toHaveBeenCalledTimes(2);
    for (const stop of stops) stop();
  });

  it('tells the engine first, whenever it subscribed', () => {
    const el = makeContainer();
    const order: string[] = [];
    onClassifiedScroll(el, () => order.push('consumer'));
    onClassifiedScrollAsEngine(el, (scroll) => { order.push('engine'); return scroll; });
    el.fireScroll();
    expect(order).toEqual(['engine', 'consumer']);
  });

  it('tells every consumer the class the engine answers with', () => {
    const el = makeContainer();
    const corrected = { navigation: true, kind: 'carry' };
    const seen: unknown[] = [];
    onClassifiedScroll(el, (scroll) => seen.push(scroll));
    onClassifiedScrollAsEngine(el, () => corrected as any);
    onClassifiedScroll(el, (scroll) => seen.push(scroll));
    el.fireScroll();
    expect(classifyScroll).toHaveBeenCalledTimes(1);
    expect(seen).toEqual([corrected, corrected]);
    expect(seen[0]).toBe(corrected);
  });

  it('tells a consumer the event\'s own class beside the engine\'s answer', () => {
    const el = makeContainer();
    const corrected = { navigation: true, kind: 'carry' };
    const heard = vi.fn();
    onClassifiedScroll(el, heard);
    onClassifiedScrollAsEngine(el, () => corrected as any);
    el.fireScroll();
    const event = classifyScroll.mock.results[0].value;
    expect(heard).toHaveBeenCalledWith(corrected, event);
    expect(heard.mock.calls[0][1]).toBe(event);
  });

  it('reports a throw to the page at once, where the platform can', () => {
    const reported = vi.fn();
    vi.stubGlobal('reportError', reported);
    vi.useFakeTimers();
    try {
      const el = makeContainer();
      const broke = new Error('consumer broke');
      const heard = vi.fn();
      onClassifiedScroll(el, () => { throw broke; });
      onClassifiedScroll(el, heard);
      el.fireScroll();
      expect(reported).toHaveBeenCalledTimes(1);
      expect(reported).toHaveBeenCalledWith(broke);
      expect(heard).toHaveBeenCalledTimes(1);
      expect(() => vi.runAllTimers()).not.toThrow();
    } finally {
      vi.useRealTimers();
      vi.unstubAllGlobals();
    }
  });

  it('keeps one DOM listener per container, and drops it with the last subscriber', () => {
    const el = makeContainer();
    const other = makeContainer();
    const stopA = onClassifiedScroll(el, () => {});
    const stopEngine = onClassifiedScrollAsEngine(el, (scroll) => scroll);
    const stopOther = onClassifiedScroll(other, () => {});
    expect(el.domListeners()).toBe(1);
    expect(other.domListeners()).toBe(1);
    stopA();
    expect(el.domListeners()).toBe(1);
    stopEngine();
    expect(el.domListeners()).toBe(0);
    stopOther();
    expect(other.domListeners()).toBe(0);
  });

  it('tells the listeners after one that throws, and still reports the throw', () => {
    vi.stubGlobal('reportError', undefined);
    vi.useFakeTimers();
    try {
      const el = makeContainer();
      const heard = vi.fn();
      onClassifiedScrollAsEngine(el, () => { throw new Error('engine broke'); });
      onClassifiedScroll(el, heard);
      el.fireScroll();
      expect(heard).toHaveBeenCalledTimes(1);
      expect(() => vi.runAllTimers()).toThrow('engine broke');
    } finally {
      vi.useRealTimers();
      vi.unstubAllGlobals();
    }
  });

  it('does not tell a listener that an earlier one unsubscribed', () => {
    const el = makeContainer();
    const late = vi.fn();
    let stopLate = () => {};
    onClassifiedScroll(el, () => stopLate());
    stopLate = onClassifiedScroll(el, late);
    el.fireScroll();
    expect(late).not.toHaveBeenCalled();
  });

  it('keeps a later registration when an old unsubscribe runs twice', () => {
    const el = makeContainer();
    const stopOld = onClassifiedScroll(el, () => {});
    stopOld();
    const heard = vi.fn();
    const stopNew = onClassifiedScroll(el, heard);
    stopOld();
    const also = vi.fn();
    onClassifiedScroll(el, also);
    expect(el.domListeners()).toBe(1);
    el.fireScroll();
    expect(heard).toHaveBeenCalledTimes(1);
    expect(also).toHaveBeenCalledTimes(1);
    stopNew();
  });

  it('keeps a later registration when a replaced engine unsubscribes late', () => {
    const el = makeContainer();
    const stopFirst = onClassifiedScrollAsEngine(el, (scroll) => scroll);
    const stopSecond = onClassifiedScrollAsEngine(el, (scroll) => scroll);
    stopSecond();
    const heard = vi.fn();
    onClassifiedScroll(el, heard);
    stopFirst();
    const also = vi.fn();
    onClassifiedScroll(el, also);
    expect(el.domListeners()).toBe(1);
    el.fireScroll();
    expect(classifyScroll).toHaveBeenCalledTimes(1);
    expect(heard).toHaveBeenCalledTimes(1);
    expect(also).toHaveBeenCalledTimes(1);
  });

  it('keeps an engine registered again when its replaced registration unsubscribes', () => {
    const el = makeContainer();
    const engine = vi.fn((scroll) => scroll);
    const stopFirst = onClassifiedScrollAsEngine(el, engine);
    onClassifiedScrollAsEngine(el, (scroll) => scroll);
    onClassifiedScrollAsEngine(el, engine);
    stopFirst();
    el.fireScroll();
    expect(engine).toHaveBeenCalledTimes(1);
  });

  it('does not tell a listener removed and registered again inside the event', () => {
    const el = makeContainer();
    const late = vi.fn();
    let stopLate = () => {};
    onClassifiedScroll(el, () => { stopLate(); onClassifiedScroll(el, late); });
    stopLate = onClassifiedScroll(el, late);
    el.fireScroll();
    expect(late).not.toHaveBeenCalled();
    el.fireScroll();
    expect(late).toHaveBeenCalledTimes(1);
  });

  it('keeps a listener registered again when its old unsubscribe runs twice', () => {
    const el = makeContainer();
    onClassifiedScroll(el, () => {});
    const heard = vi.fn();
    const stopOld = onClassifiedScroll(el, heard);
    stopOld();
    onClassifiedScroll(el, heard);
    stopOld();
    el.fireScroll();
    expect(heard).toHaveBeenCalledTimes(1);
  });

  it('does not tell a container about another container\'s scroll', () => {
    const el = makeContainer();
    const other = makeContainer();
    const heard = vi.fn();
    onClassifiedScroll(other, heard);
    onClassifiedScroll(el, () => {});
    el.fireScroll();
    expect(heard).not.toHaveBeenCalled();
  });
});

function code(rel: string): string {
  return readFileSync(srcPath(rel), 'utf8')
    .replace(/\/\*[\s\S]*?\*\//g, ' ')
    .replace(/^\s*\/\/.*$/gm, ' ');
}

describe('every scroll consumer hears the classified listener', () => {
  const CONSUMERS = [
    'hooks/useScrollMemory.ts',
    'hooks/useHideOnScroll.ts',
    'hooks/useThreadScrollIndicator.ts',
    'components/chat/ThreadView.tsx',
  ];

  it.each(CONSUMERS)('%s subscribes, and adds no scroll listener of its own', (rel) => {
    const source = code(rel);
    expect(source).toContain('onClassifiedScroll(');
    expect(source).not.toMatch(/addEventListener\(\s*'scroll'/);
  });

  it('reads the indicator\'s navigation from the event\'s own class', () => {
    const source = code('hooks/useThreadScrollIndicator.ts');
    expect(source).toMatch(/function onScroll\(_told: ScrollClass, event: ScrollClass\)/);
    expect(source).toContain('programmaticScroll: event.navigation,');
  });

  it('attaches the scroll engine as the engine', () => {
    const source = code('components/chat/CreateThreadView.tsx');
    expect(source).toContain('onClassifiedScrollAsEngine(el, onScroll)');
    expect(source).not.toMatch(/addEventListener\(\s*'scroll'/);
  });

  it.each(CONSUMERS.filter((rel) => rel !== 'hooks/useScrollMemory.ts'))(
    '%s classifies nothing itself',
    (rel) => { expect(code(rel)).not.toContain('classifyScroll('); },
  );
});
