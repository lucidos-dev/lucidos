import { describe, it, expect, beforeEach, afterEach, vi } from 'vitest';
// @ts-expect-error: Node APIs available at runtime via Vitest, no @types/node in project
import { readFileSync } from 'node:fs';
// @ts-expect-error: same
import { dirname, resolve } from 'node:path';
// @ts-expect-error: same
import { fileURLToPath } from 'node:url';
import {
  BARS_SHOWN,
  chromeOffsetPx,
  headerOffsetPx,
  landWithoutGlide,
  nextBarsIntent,
  promptCanSlide,
  promptOffsetPx,
  shouldKeepHeaderVisible,
  countsAsReaderScroll,
  revealsBars,
  spacerHeightPx,
  titleBarOffsetPx,
  type BarsIntent,
} from './useHideOnScroll';
import type { NavScrollKind } from '../components/chat/scrollState';

const THRESHOLD = 12;

describe('nextBarsIntent (dynamic bars)', () => {
  function fold(deltas: number[], from: BarsIntent = BARS_SHOWN): BarsIntent {
    return deltas.reduce((intent, delta) => nextBarsIntent(intent, delta, THRESHOLD), from);
  }

  it('sends the bars away after a threshold of travel down, and back after one up', () => {
    const away = fold([THRESHOLD]);
    expect(away.away).toBe(true);
    expect(fold([-THRESHOLD], away).away).toBe(false);
  });

  it('adds up travel in one direction across events', () => {
    expect(fold([5, 5]).away).toBe(false);
    expect(fold([5, 5, 5]).away).toBe(true);
  });

  it('never flips on jitter, since a reversal starts the travel afresh', () => {
    expect(fold([8, -8, 8, -8, 8]).away).toBe(false);
    const away = fold([100]);
    expect(fold([-8, 8, -8, 8, -8], away).away).toBe(true);
  });

  it('changes nothing for a zero delta', () => {
    const away = fold([100]);
    expect(nextBarsIntent(away, 0, THRESHOLD)).toBe(away);
  });
});

describe('headerOffsetPx (dynamic bars)', () => {
  const chromeHeight = 90;

  it('moves the header and title bar their whole height away, or not at all', () => {
    expect(headerOffsetPx({ away: true, scrollTop: 500, chromeHeight })).toBe(-chromeHeight);
    expect(headerOffsetPx({ away: false, scrollTop: 500, chromeHeight })).toBe(0);
  });

  it('keeps them shown within their own height of the top', () => {
    expect(headerOffsetPx({ away: true, scrollTop: 0, chromeHeight })).toBe(0);
    expect(headerOffsetPx({ away: true, scrollTop: chromeHeight, chromeHeight })).toBe(0);
    expect(headerOffsetPx({ away: true, scrollTop: chromeHeight + 1, chromeHeight })).toBe(-chromeHeight);
  });
});

describe('chromeOffsetPx (dynamic bars)', () => {
  const headerHeight = 110;
  const controlsTop = 47;
  const titleBarHeight = 40;
  const base = { disabled: false, keyboardOpen: false, away: false, scrollTop: 500, headerHeight, titleBarHeight };

  it('sends the title bar away with the header while the keyboard is up', () => {
    expect(chromeOffsetPx({ ...base, keyboardOpen: true })).toBe(-(headerHeight + titleBarHeight));
  });

  it('keeps the title bar off screen on the first frame of the keyboard close', () => {
    const offset = chromeOffsetPx({ ...base, keyboardOpen: true });
    expect(headerHeight + offset + titleBarHeight).toBeLessThanOrEqual(0);
    expect(controlsTop + offset + titleBarHeight).toBeLessThanOrEqual(0);
  });

  it('follows the scroll with the keyboard down, and pins wins over both', () => {
    expect(chromeOffsetPx({ ...base, away: true })).toBe(-(headerHeight + titleBarHeight));
    expect(chromeOffsetPx(base)).toBe(0);
    expect(chromeOffsetPx({ ...base, disabled: true, keyboardOpen: true, away: true })).toBe(0);
  });
});

describe('titleBarOffsetPx (dynamic bars)', () => {
  const headerHeight = 110;
  const titleBarHeight = 40;
  const chrome = headerHeight + titleBarHeight;
  const elsewhere = { onOwnPane: false, headerHeight, titleBarHeight };
  const hookSource = readFileSync(
    resolve(dirname(fileURLToPath(import.meta.url)), 'useHideOnScroll.ts'),
    'utf8',
  );

  it('takes the chrome offset on its own pane', () => {
    for (const chromeOffset of [0, -chrome]) {
      expect(titleBarOffsetPx({ ...elsewhere, onOwnPane: true, chromeOffset, paneScrollTop: 0 })).toBe(chromeOffset);
    }
  });

  it('goes off screen with the header on another pane, over a scrolled transcript', () => {
    const offset = titleBarOffsetPx({ ...elsewhere, chromeOffset: -headerHeight, paneScrollTop: 500 });
    expect(headerHeight + offset + titleBarHeight).toBeLessThanOrEqual(0);
  });

  it('stays shown until the transcript can reach the band above it', () => {
    expect(titleBarOffsetPx({ ...elsewhere, chromeOffset: -headerHeight, paneScrollTop: 0 })).toBe(0);
    expect(titleBarOffsetPx({ ...elsewhere, chromeOffset: -headerHeight, paneScrollTop: titleBarHeight })).toBe(0);
    expect(titleBarOffsetPx({ ...elsewhere, chromeOffset: -headerHeight, paneScrollTop: titleBarHeight + 1 })).toBe(-chrome);
  });

  it('stays shown whenever the header is shown', () => {
    expect(titleBarOffsetPx({ ...elsewhere, chromeOffset: 0, paneScrollTop: 500 })).toBe(0);
  });

  it('is bound from the thread pane, not from the current pane', () => {
    expect(hookSource).toMatch(
      /const nextTitleBar = document\.querySelector<HTMLElement>\(`\$\{SCROLL_SELECTORS\.thread\} \.mobile-thread-title-row`\)/,
    );
    expect(hookSource).not.toMatch(/container\?\.querySelector\('\.mobile-thread-title-row'\)/);
  });

  it('lands a freshly bound title bar instead of gliding it from its resting place', () => {
    expect(hookSource).toMatch(/titleBarFresh = el !== null;/);
    expect(hookSource).toMatch(/if \(titleBarEl && titleBarFresh\) landWithoutGlide\(titleBarEl\);/);
  });

  it('re-applies the offset when the title bar resizes', () => {
    expect(hookSource).toMatch(
      /new ResizeObserver\(\(\) => \{\s*updateTitleBarHeightVar\(\);\s*applyTransform\(\);\s*\}\)/,
    );
  });
});

describe('promptOffsetPx (dynamic bars)', () => {
  const promptHeight = 80;
  const far = 10_000;
  const onPane = { onThreadPane: true, chromeOffset: 0, promptHeight };
  const elsewhere = { onThreadPane: false, away: false, promptHeight };
  const hookSource = readFileSync(
    resolve(dirname(fileURLToPath(import.meta.url)), 'useHideOnScroll.ts'),
    'utf8',
  );

  it('moves the prompt its whole height away, or not at all', () => {
    expect(promptOffsetPx({ ...onPane, away: true, distanceToBottom: far })).toBe(promptHeight);
    expect(promptOffsetPx({ ...onPane, away: false, distanceToBottom: far })).toBe(0);
  });

  it('follows the reader on its own pane, even where the header stays shown', () => {
    expect(promptOffsetPx({ ...onPane, away: true, chromeOffset: 0, distanceToBottom: far })).toBe(promptHeight);
  });

  it('keeps it shown within its own height of the bottom, whoever put the reader there', () => {
    expect(promptOffsetPx({ ...onPane, away: true, distanceToBottom: promptHeight })).toBe(0);
    expect(promptOffsetPx({ ...onPane, away: true, distanceToBottom: 0 })).toBe(0);
    expect(promptOffsetPx({ ...onPane, away: true, distanceToBottom: promptHeight + 1 })).toBe(promptHeight);
  });

  it('goes away with the header on another pane, so the two glide back together', () => {
    expect(promptOffsetPx({ ...elsewhere, chromeOffset: -110, distanceToBottom: far })).toBe(promptHeight);
    expect(promptOffsetPx({ ...elsewhere, chromeOffset: 0, distanceToBottom: far })).toBe(0);
  });

  it('ignores the intent off its pane, which belongs to the current pane', () => {
    expect(promptOffsetPx({ ...elsewhere, away: true, chromeOffset: 0, distanceToBottom: far })).toBe(0);
  });

  it('stays shown off its pane over a transcript at its live edge', () => {
    expect(promptOffsetPx({ ...elsewhere, chromeOffset: -110, distanceToBottom: promptHeight })).toBe(0);
  });

  it('measures the thread transcript, not the current pane, while off its pane', () => {
    expect(hookSource).toMatch(/threadScroller = document\.querySelector\(SCROLL_SELECTORS\.thread\)/);
    expect(hookSource).toMatch(
      /if \(!onThreadPane && offset < 0\) \{\s*threadScrollTop = threadScroller \? clampedScrollTop\(threadScroller\) : 0;\s*threadDistanceToBottom = threadScroller \? distanceToBottom\(threadScroller, threadScrollTop\) : 0;/,
    );
  });

  it('binds the down chevron from the thread pane, so it rides the prompt on every pane', () => {
    expect(hookSource).toMatch(/threadScroller\?\.parentElement\?\.querySelector\(':scope > \.scroll-to-bottom'\)/);
  });
});

describe('countsAsReaderScroll (dynamic bars)', () => {
  it("counts the reader's own scroll", () => {
    expect(countsAsReaderScroll({ navigation: false, headerPinned: false })).toBe(true);
  });

  it('re-bases for every write of ours, and while a deep link pins the header', () => {
    expect(countsAsReaderScroll({ navigation: true, headerPinned: false })).toBe(false);
    expect(countsAsReaderScroll({ navigation: false, headerPinned: true })).toBe(false);
  });
});

describe('revealsBars (dynamic bars)', () => {
  it('brings the bars back for a write that takes the reader somewhere', () => {
    expect(revealsBars({ kind: 'placement', moved: true })).toBe(true);
    expect(revealsBars({ kind: 'held', moved: true })).toBe(true);
  });

  it('leaves them for an anchor write and the follow carrying a reply', () => {
    expect(revealsBars({ kind: 'anchor', moved: true })).toBe(false);
    expect(revealsBars({ kind: 'carry', moved: true })).toBe(false);
  });

  it('leaves them for a write that moved nothing, like a tap focusing a control', () => {
    expect(revealsBars({ kind: 'placement', moved: false })).toBe(false);
    expect(revealsBars({ kind: 'held', moved: false })).toBe(false);
  });
});

describe('promptCanSlide (dynamic bars)', () => {
  it('slides only with the bars dynamic, no keyboard, and a real thread', () => {
    expect(promptCanSlide({ disabled: false, keyboardOpen: false, composeEmpty: false })).toBe(true);
  });

  it('stays shown while pinned, while typing, and in the compose-empty view', () => {
    expect(promptCanSlide({ disabled: true, keyboardOpen: false, composeEmpty: false })).toBe(false);
    expect(promptCanSlide({ disabled: false, keyboardOpen: true, composeEmpty: false })).toBe(false);
    expect(promptCanSlide({ disabled: false, keyboardOpen: false, composeEmpty: true })).toBe(false);
  });
});

describe('--mobile-header-offset stays off the document root', () => {
  const hookSource = readFileSync(
    resolve(dirname(fileURLToPath(import.meta.url)), 'useHideOnScroll.ts'),
    'utf8',
  );

  it('never sets the offset on documentElement', () => {
    const rootWrites = hookSource.match(
      /documentElement\.style\.setProperty\(\s*['"]--mobile-header-offset/g,
    ) ?? [];
    expect(rootWrites).toHaveLength(0);
  });

  it('writes the offset on every consumer element', () => {
    expect(hookSource).toMatch(/titleBarEl\?\.style\.setProperty\(\s*['"]--mobile-header-offset/);
    expect(hookSource).toMatch(/chevronEl\?\.style\.setProperty\(\s*['"]--mobile-header-offset/);
  });

  const mobileCss = readFileSync(
    resolve(dirname(fileURLToPath(import.meta.url)), '../styles/mobile.css'),
    'utf8',
  );

  it('moves the down chevron with the prompt by translate, never by a layout property', () => {
    const offsetUses = mobileCss.split('\n').filter((line: string) => line.includes('--mobile-prompt-offset'));
    expect(offsetUses.length).toBeGreaterThan(0);
    for (const line of offsetUses) expect(line).toMatch(/^\s*translate: 0 .*var\(--mobile-prompt-offset/);
  });

  it('keeps the prompt offset off the root too, on the prompt and the down chevron', () => {
    expect(hookSource).not.toMatch(/documentElement\.style\.setProperty\(\s*['"]--mobile-prompt-offset/);
    expect(hookSource).toMatch(/promptEl\.style\.translate = promptOffsetRem/);
    expect(hookSource).toMatch(/downChevronEl\?\.style\.setProperty\(\s*['"]--mobile-prompt-offset/);
    expect(hookSource).toMatch(/promptEl\.style\.setProperty\(\s*['"]--mobile-prompt-offset/);
  });

  it('moves every bar by `translate` alone, never by `transform`', () => {
    expect(hookSource).not.toMatch(/\.style\.transform\s*=/);
    expect(hookSource).toMatch(/headerRef\.current\.style\.translate = /);
  });

  it('gates the navigation reveal on the anchor and carry kinds and the anchored position', () => {
    expect(hookSource).toMatch(/countsAsReaderScroll\(\{\s*navigation: scroll\.page\.navigation,\s*headerPinned:/);
    expect(hookSource).toMatch(/if \(scroll\.page\.kind !== 'anchor' && scroll\.page\.kind !== 'carry' && !atRebasedTop\) revealBars\(\);/);
  });

  it('reveals on a pane swipe before it rebinds, so the outgoing bars glide in too', () => {
    expect(hookSource).toMatch(
      /if \(container !== currentContainer\) \{\s*revealBars\(\);\s*applyTransform\(\);\s*\}[\s\S]*?bindOffsetConsumers\(container\)/,
    );
  });

  it('writes scrollTop only through holdAcrossRelayout, so a send landing survives the keyboard', () => {
    expect(hookSource).not.toMatch(/\.scrollTop\s*[+-]?=(?!=)/);
    expect(hookSource.match(/holdAcrossRelayout\(currentContainer/g) ?? []).toHaveLength(3);
  });

  it('re-takes its baseline at every write of ours, not on the scroll event', () => {
    expect(hookSource).toMatch(/onRebasedScroll\(\(el, kind\) => \{[\s\S]*prevScrollTop = clampedScrollTop\(el\)/);
  });

  it('decides the bars at the write, from its kind', () => {
    expect(hookSource).toMatch(
      /onRebasedScroll\(\(el, kind\) => \{[\s\S]*?const moved = Math\.abs\(top - prevScrollTop\) > 1;\s*prevScrollTop = top;[\s\S]*?if \(revealsBars\(\{ kind, moved \}\)\) \{\s*revealBars\(\);/,
    );
  });

  it('re-takes it again on the settling frame, and cancels that frame on teardown', () => {
    expect(hookSource).toMatch(/rebaseSettleRaf = requestAnimationFrame\(\(\) => \{[\s\S]*prevScrollTop = clampedScrollTop\(el\)/);
    expect(hookSource).toMatch(/if \(rebaseSettleRaf !== null\) cancelAnimationFrame\(rebaseSettleRaf\);[\s\S]*observer\.disconnect\(\)/);
  });

  it('keeps the offset off `top`, which would reinstate the forced layout', () => {
    const offsetOnTop = mobileCss.match(/top:[^;]*--mobile-header-offset/g) ?? [];
    expect(offsetOnTop).toHaveLength(0);
    const offsetOnTransform = mobileCss.match(/transform:[^;]*--mobile-header-offset/g) ?? [];
    expect(offsetOnTransform).toHaveLength(0);
    expect(mobileCss.match(/translate:[^;]*--mobile-header-offset/g) ?? []).toHaveLength(2);
  });
});

describe('the dynamic bars glide on a compositor transition', () => {
  const read = (rel: string): string =>
    readFileSync(resolve(dirname(fileURLToPath(import.meta.url)), rel), 'utf8');
  const mobileCss = read('../styles/mobile.css');
  const inputCss = read('../styles/chat/input-messages.css');
  const GLIDE = /translate var\(--bars-glide-duration, 0s\) ease/;

  function declarationOf(css: string, selector: string, property: string): string {
    const normalize = (sel: string) => sel.replace(/\s+/g, ' ').trim();
    const uncommented = css.replace(/\/\*[\s\S]*?\*\//g, '');
    const rules = [...uncommented.matchAll(/([^{}]+)\{([^}]*)\}/g)]
      .filter(m => m[1].split(',').map(normalize).includes(normalize(selector)));
    expect(rules.length, `no rule for ${selector}`).toBeGreaterThan(0);
    let value = '';
    for (const rule of rules) {
      const found = new RegExp(`(?:^|[;\\s])${property}:([^;]*);`).exec(rule[2]);
      if (found) value = found[1];
    }
    return value;
  }
  const transitionOf = (css: string, selector: string) => declarationOf(css, selector, 'transition');

  it('gives every bar its layer from the start, the prompt and the app panel included', () => {
    for (const selector of [
      '.app-header',
      '.mobile-swipe-pane .content-pane-body.has-app-ui .app-ui-inline',
      '.mobile-swipe-pane .mobile-thread-title-row',
      ':root[data-mobile-dynamic-bars] .thread-pane:not(.compose-empty) .prompt-area',
    ]) {
      expect(declarationOf(mobileCss, selector, 'will-change').trim()).toBe('translate');
    }
  });

  it('gives every bar the glide on `translate`', () => {
    expect(transitionOf(mobileCss, '.app-header')).toMatch(GLIDE);
    expect(transitionOf(mobileCss, '.mobile-swipe-pane .mobile-thread-title-row')).toMatch(GLIDE);
    expect(transitionOf(mobileCss, ':root[data-mobile-dynamic-bars] .thread-pane:not(.compose-empty) .prompt-area')).toMatch(GLIDE);
    expect(transitionOf(inputCss, '.scroll-to-top')).toMatch(GLIDE);
    expect(transitionOf(inputCss, '.scroll-to-bottom')).toMatch(GLIDE);
  });

  it('runs the glide only while the bars are dynamic', () => {
    const declarations = [mobileCss, inputCss].join('\n').match(/--bars-glide-duration:[^;]*;/g) ?? [];
    expect(declarations).toEqual(['--bars-glide-duration: var(--duration-slow);']);
    expect(mobileCss).toMatch(/:root\[data-mobile-dynamic-bars\] \{\s*--bars-glide-duration: var\(--duration-slow\);\s*\}/);
    const uses = [mobileCss, inputCss].join('\n').match(/var\(--bars-glide-duration[^)]*\)/g) ?? [];
    for (const use of uses) expect(use).toBe('var(--bars-glide-duration, 0s)');
  });
});

describe('a prompt about to take focus lands without its glide', () => {
  const read = (rel: string): string =>
    readFileSync(resolve(dirname(fileURLToPath(import.meta.url)), rel), 'utf8');

  afterEach(() => {
    vi.unstubAllGlobals();
  });

  it('flushes style with the transition off, then restores it', () => {
    const el = { style: { transition: '' } } as unknown as HTMLElement;
    const transitionAtFlush: string[] = [];
    vi.stubGlobal('getComputedStyle', (target: HTMLElement) => {
      transitionAtFlush.push(target.style.transition);
      return { translate: 'none' };
    });

    landWithoutGlide(el);

    expect(transitionAtFlush).toEqual(['none']);
    expect(el.style.transition).toBe('');
  });

  it('is what the focus path asks for, before it focuses', () => {
    expect(read('../components/chat/promptFocus.ts')).toMatch(
      /new CustomEvent<RevealBarsDetail>\('reveal-mobile-bars', \{ detail: \{ instant: true \} \}\)\);\s*el\.focus\(/,
    );
    expect(read('useHideOnScroll.ts')).toMatch(/detail\?\.instant\) landWithoutGlide\(promptEl\);/);
  });
});

function createScrollTracker(
  getActiveElement: () => { tagName: string; pane?: string } | null,
  isNavigationScroll: () => boolean = () => false,
  isRepaintNudging: () => boolean = () => false,
  isUserScrolling: () => boolean = () => false,
  isAnchorScroll: () => boolean = () => false,
  isCarryScroll: () => boolean = () => false,
) {
  let prevScrollTop = 0;
  let viewScrollTop = 0;
  let intent = BARS_SHOWN;
  let rebasedTop = -1;
  const cachedHeight = 48;
  let currentPane: string | null = null;
  let keyboardOpen = false;
  let disabled = false;

  function setCurrentPane(pane: string | null) {
    currentPane = pane;
  }

  function applyScrollDelta(scrollTop: number, scrollHeight: number, clientHeight: number) {
    if (disabled) return;
    if (isRepaintNudging() && !isUserScrolling()) return;

    const maxScroll = Math.max(0, scrollHeight - clientHeight);
    const clamped = Math.min(Math.max(0, scrollTop), maxScroll);

    const atRebasedTop = rebasedTop >= 0 && Math.abs(clamped - rebasedTop) <= 1;
    if (!atRebasedTop) rebasedTop = -1;

    if (!countsAsReaderScroll({ navigation: isNavigationScroll(), headerPinned: false })) {
      prevScrollTop = clamped;
      viewScrollTop = clamped;
      if (!isAnchorScroll() && !isCarryScroll() && !atRebasedTop) intent = BARS_SHOWN;
      return;
    }

    const active = getActiveElement();
    if (active && (active.tagName === 'TEXTAREA' || active.tagName === 'INPUT' || active.tagName === 'SELECT')) {
      if (active.pane === currentPane) return;
    }

    intent = nextBarsIntent(intent, clamped - prevScrollTop, THRESHOLD);
    prevScrollTop = clamped;
    viewScrollTop = clamped;
  }

  function switchContainer(containerScrollTop: number | null) {
    rebasedTop = -1;
    intent = BARS_SHOWN;
    prevScrollTop = Math.max(0, containerScrollTop ?? 0);
    viewScrollTop = prevScrollTop;
  }

  function rebase(scrollTop: number, scrollHeight: number, clientHeight: number, kind: NavScrollKind = 'carry') {
    const maxScroll = Math.max(0, scrollHeight - clientHeight);
    const top = Math.min(Math.max(0, scrollTop), maxScroll);
    const moved = Math.abs(top - prevScrollTop) > 1;
    prevScrollTop = top;
    rebasedTop = top;
    if (revealsBars({ kind, moved })) {
      intent = BARS_SHOWN;
      viewScrollTop = top;
    }
  }

  function syncToScroll(containerScrollTop: number) {
    prevScrollTop = Math.max(0, containerScrollTop);
    viewScrollTop = prevScrollTop;
    intent = BARS_SHOWN;
  }

  function onFocusIn(containerScrollTop: number) {
    keyboardOpen = true;
    prevScrollTop = Math.max(0, containerScrollTop);
  }

  function correctForScrollPosition(containerScrollTop: number) {
    viewScrollTop = Math.max(0, containerScrollTop);
  }

  function headerOffset(): number {
    return headerOffsetPx({ away: intent.away, scrollTop: viewScrollTop, chromeHeight: cachedHeight });
  }

  function getEffectiveOffset(): number {
    if (disabled) return 0;
    return keyboardOpen ? -cachedHeight : headerOffset();
  }

  function setDisabled(value: boolean) {
    disabled = value;
  }

  function setKeyboardOpen(open: boolean) {
    keyboardOpen = open;
  }

  function recoverKeyboardState(isTextInputFocused: boolean) {
    if (keyboardOpen && !isTextInputFocused) {
      keyboardOpen = false;
    }
  }

  return {
    applyScrollDelta,
    rebase,
    switchContainer,
    syncToScroll,
    onFocusIn,
    correctForScrollPosition,
    setCurrentPane,
    setKeyboardOpen,
    recoverKeyboardState,
    get headerOffset() { return headerOffset(); },
    get keyboardOpen() { return keyboardOpen; },
    get disabled() { return disabled; },
    getEffectiveOffset,
    setDisabled,
  };
}

describe('useHideOnScroll keyboard suppression', () => {
  let mockActive: { tagName: string; pane?: string } | null = null;
  let tracker: ReturnType<typeof createScrollTracker>;

  beforeEach(() => {
    mockActive = null;
    tracker = createScrollTracker(() => mockActive);
    tracker.setCurrentPane('threads');
  });

  it('hides header on scroll-down when no input focused', () => {
    tracker.applyScrollDelta(0, 1000, 500);
    tracker.applyScrollDelta(100, 1000, 500);
    expect(tracker.headerOffset).toBe(-48);
  });

  it('ignores scroll when textarea has focus in same pane', () => {
    mockActive = { tagName: 'TEXTAREA', pane: 'threads' };

    tracker.applyScrollDelta(0, 1000, 500);
    tracker.applyScrollDelta(50, 1000, 500);
    tracker.applyScrollDelta(100, 800, 300);

    expect(tracker.headerOffset).toBe(0);
  });

  it('shows the bars after blur, then hides them on the next scroll down', () => {
    mockActive = { tagName: 'TEXTAREA', pane: 'threads' };

    tracker.applyScrollDelta(50, 1000, 500);
    expect(tracker.headerOffset).toBe(0);

    mockActive = null;
    tracker.syncToScroll(50);
    expect(tracker.headerOffset).toBe(0);

    tracker.applyScrollDelta(80, 1000, 500);
    expect(tracker.headerOffset).toBe(-48);
  });

  it('ignores scroll with input element focused in same pane', () => {
    mockActive = { tagName: 'INPUT', pane: 'threads' };

    tracker.applyScrollDelta(0, 1000, 500);
    tracker.applyScrollDelta(100, 1000, 500);
    expect(tracker.headerOffset).toBe(0);
  });

  it('hides header when input gains focus (keyboard opens)', () => {
    expect(tracker.getEffectiveOffset()).toBe(0);

    tracker.onFocusIn(0);
    expect(tracker.getEffectiveOffset()).toBe(-48);
  });

  it('allows scroll when textarea has focus in a DIFFERENT pane (iOS swipe)', () => {
    tracker.setCurrentPane('threads');
    mockActive = { tagName: 'TEXTAREA', pane: 'thread' };

    tracker.applyScrollDelta(0, 1000, 500);
    tracker.applyScrollDelta(100, 1000, 500);
    expect(tracker.headerOffset).toBe(-48);
  });

  it('reveals the bars on a pane swipe, and again on the swipe back', () => {
    tracker.switchContainer(0);
    tracker.applyScrollDelta(100, 1000, 500);
    expect(tracker.headerOffset).toBe(-48);

    tracker.switchContainer(0);
    expect(tracker.headerOffset).toBe(0);

    tracker.switchContainer(400);
    expect(tracker.headerOffset).toBe(0);
  });

  it('keeps the bars shown on arrival until the reader scrolls down', () => {
    tracker.switchContainer(200);
    expect(tracker.headerOffset).toBe(0);

    tracker.applyScrollDelta(230, 1000, 500);
    expect(tracker.headerOffset).toBe(-48);

    tracker.applyScrollDelta(210, 1000, 500);
    expect(tracker.headerOffset).toBe(0);
  });

  it('resets header when container disappears (null)', () => {
    tracker.switchContainer(0);
    tracker.applyScrollDelta(100, 1000, 500);
    expect(tracker.headerOffset).toBe(-48);

    tracker.switchContainer(null);
    expect(tracker.headerOffset).toBe(0);
  });

  it('keeps header visible when switching to a container at top', () => {
    expect(tracker.headerOffset).toBe(0);
    tracker.switchContainer(0);
    expect(tracker.headerOffset).toBe(0);
  });
});

describe('useHideOnScroll during one of our own navigations', () => {
  let mockActive: { tagName: string; pane?: string } | null = null;
  let navigating = false;

  beforeEach(() => {
    mockActive = null;
    navigating = false;
  });

  it('resets the header to visible while a navigation is gliding', () => {
    const tracker = createScrollTracker(() => mockActive, () => navigating);
    tracker.switchContainer(0);

    tracker.applyScrollDelta(100, 2000, 500);
    expect(tracker.headerOffset).toBe(-48);

    navigating = true;
    tracker.applyScrollDelta(1500, 2000, 500);

    expect(tracker.headerOffset).toBe(0);
  });

  it('resumes normal scroll tracking once the tween lands', () => {
    const tracker = createScrollTracker(() => mockActive, () => navigating);
    tracker.switchContainer(0);

    navigating = true;
    tracker.applyScrollDelta(1500, 2000, 500);
    expect(tracker.headerOffset).toBe(0);

    navigating = false;

    tracker.applyScrollDelta(1470, 2000, 500);
    tracker.applyScrollDelta(1500, 2000, 500);
    expect(tracker.headerOffset).toBe(-48);
  });

  it('keeps the header visible across a multi-frame glide', () => {
    const tracker = createScrollTracker(() => mockActive, () => navigating);

    navigating = true;
    tracker.switchContainer(0);

    tracker.applyScrollDelta(500, 1000, 500);
    expect(tracker.headerOffset).toBe(0);

    tracker.applyScrollDelta(1500, 2000, 500);
    expect(tracker.headerOffset).toBe(0);

    navigating = false;

    tracker.applyScrollDelta(1470, 2000, 500);
    tracker.applyScrollDelta(1500, 2000, 500);
    expect(tracker.headerOffset).toBe(-48);
  });
});

describe('useHideOnScroll as a thread opens at its saved place', () => {
  it('keeps the bars shown however late the placement event lands', () => {
    const tracker = createScrollTracker(() => null);
    tracker.switchContainer(100);
    tracker.rebase(2500, 4000, 800, 'placement');
    tracker.applyScrollDelta(2500, 4000, 800);
    expect(tracker.headerOffset).toBe(0);
  });

  it('brings away bars back at the write', () => {
    const tracker = createScrollTracker(() => null);
    tracker.switchContainer(0);
    tracker.applyScrollDelta(300, 4000, 800);
    expect(tracker.headerOffset).toBe(-48);
    tracker.rebase(2500, 4000, 800, 'placement');
    expect(tracker.headerOffset).toBe(0);
  });

  it('keeps away bars away when a tap focuses a control already on screen', () => {
    const tracker = createScrollTracker(() => null);
    tracker.switchContainer(0);
    tracker.applyScrollDelta(300, 4000, 800);
    tracker.rebase(300, 4000, 800, 'placement');
    expect(tracker.headerOffset).toBe(-48);
  });

  it("measures the reader's next scroll from the placement", () => {
    const tracker = createScrollTracker(() => null);
    tracker.switchContainer(100);
    tracker.rebase(2500, 4000, 800, 'held');
    tracker.applyScrollDelta(2530, 4000, 800);
    expect(tracker.headerOffset).toBe(-48);
  });
});

describe("useHideOnScroll under the standing follow's carry", () => {
  let carrying = false;
  beforeEach(() => { carrying = false; });

  function trackerUnderRide() {
    return createScrollTracker(() => null, () => carrying, () => false, () => false, () => false, () => carrying);
  }

  it('keeps shown bars shown as the ride carries the reader down', () => {
    const tracker = trackerUnderRide();
    tracker.switchContainer(1000);
    carrying = true;
    tracker.applyScrollDelta(1100, 3000, 500);
    tracker.applyScrollDelta(1400, 3000, 500);
    expect(tracker.headerOffset).toBe(0);
  });

  it('keeps away bars away as the ride carries the reader down', () => {
    const tracker = trackerUnderRide();
    tracker.switchContainer(1000);
    tracker.applyScrollDelta(1100, 3000, 500);
    expect(tracker.headerOffset).toBe(-48);
    carrying = true;
    tracker.applyScrollDelta(1400, 3000, 500);
    expect(tracker.headerOffset).toBe(-48);
  });

  it('ignores a carry whose scroll event lands after the navigation window', () => {
    const tracker = trackerUnderRide();
    tracker.switchContainer(1000);
    tracker.rebase(1400, 3000, 500);
    tracker.applyScrollDelta(1400, 3000, 500);
    expect(tracker.headerOffset).toBe(0);
  });

  it("measures the reader's next scroll from where the carry left them", () => {
    const tracker = trackerUnderRide();
    tracker.switchContainer(1000);
    carrying = true;
    tracker.applyScrollDelta(1400, 3000, 500);
    carrying = false;
    tracker.applyScrollDelta(1405, 3000, 500);
    expect(tracker.headerOffset).toBe(0);
  });
});

describe('useHideOnScroll iOS repaint nudge suppression', () => {
  let mockActive: { tagName: string; pane?: string } | null = null;
  let nudging = false;
  let userScrolling = false;

  beforeEach(() => {
    mockActive = null;
    nudging = false;
    userScrolling = false;
  });

  function trackerWithNudge() {
    return createScrollTracker(
      () => mockActive,
      () => false,
      () => nudging,
      () => userScrolling,
    );
  }

  function nudgeRoundTrip(
    tracker: ReturnType<typeof createScrollTracker>,
    scrollTop: number,
    onFirstLeg?: () => void,
  ) {
    nudging = true;
    tracker.applyScrollDelta(scrollTop - 1, 2000, 500);
    onFirstLeg?.();
    tracker.applyScrollDelta(scrollTop, 2000, 500);
    nudging = false;
  }

  it('does not move the header on the nudge leg itself', () => {
    const tracker = trackerWithNudge();
    tracker.switchContainer(0);
    tracker.applyScrollDelta(400, 2000, 500);
    expect(tracker.headerOffset).toBe(-48);

    let midNudge = NaN;
    nudgeRoundTrip(tracker, 400, () => { midNudge = tracker.headerOffset; });

    expect(midNudge).toBe(-48);
    expect(tracker.headerOffset).toBe(-48);
  });

  it('does not leave a fully visible header oscillating by a pixel', () => {
    const tracker = trackerWithNudge();
    tracker.switchContainer(0);
    tracker.applyScrollDelta(400, 2000, 500);
    tracker.applyScrollDelta(300, 2000, 500);
    expect(tracker.headerOffset).toBe(0);

    for (let i = 0; i < 20; i++) nudgeRoundTrip(tracker, 300);

    expect(tracker.headerOffset).toBe(0);
  });

  it('folds a real scroll that races the nudge into the next event', () => {
    const tracker = trackerWithNudge();
    tracker.switchContainer(0);
    tracker.applyScrollDelta(400, 2000, 500);
    tracker.applyScrollDelta(300, 2000, 500);
    expect(tracker.headerOffset).toBe(0);

    nudging = true;
    tracker.applyScrollDelta(400, 2000, 500);
    nudging = false;
    tracker.applyScrollDelta(400, 2000, 500);

    expect(tracker.headerOffset).toBe(-48);
  });

  it('still tracks a real scroll once the window closes', () => {
    const tracker = trackerWithNudge();
    tracker.switchContainer(0);
    nudgeRoundTrip(tracker, 1);

    tracker.applyScrollDelta(100, 2000, 500);
    expect(tracker.headerOffset).toBe(-48);
  });

  it('never suppresses a live drag, even inside the nudge window', () => {
    const tracker = trackerWithNudge();
    tracker.switchContainer(0);

    nudging = true;
    userScrolling = true;
    tracker.applyScrollDelta(100, 2000, 500);

    expect(tracker.headerOffset).toBe(-48);
  });

  it('suppresses again once the drag window lapses', () => {
    const tracker = trackerWithNudge();
    tracker.switchContainer(0);
    userScrolling = true;
    tracker.applyScrollDelta(400, 2000, 500);
    expect(tracker.headerOffset).toBe(-48);

    userScrolling = false;
    nudgeRoundTrip(tracker, 400, () => {
      expect(tracker.headerOffset).toBe(-48);
    });
    expect(tracker.headerOffset).toBe(-48);
  });
});

describe('useHideOnScroll DOM mutation correction', () => {
  let mockActive: { tagName: string; pane?: string } | null = null;
  let tracker: ReturnType<typeof createScrollTracker>;

  beforeEach(() => {
    mockActive = null;
    tracker = createScrollTracker(() => mockActive);
  });

  it('recovers header when content shrinks and scrollTop drops near top', () => {
    tracker.switchContainer(0);

    tracker.applyScrollDelta(0, 1000, 500);
    tracker.applyScrollDelta(100, 1000, 500);
    expect(tracker.headerOffset).toBe(-48);

    tracker.correctForScrollPosition(0);
    expect(tracker.headerOffset).toBe(0);
  });

  it('recovers header after keyboard dismiss when content is short', () => {
    tracker.switchContainer(0);

    tracker.applyScrollDelta(0, 1000, 500);
    tracker.applyScrollDelta(500, 1000, 500);
    expect(tracker.headerOffset).toBe(-48);

    tracker.syncToScroll(500);
    expect(tracker.headerOffset).toBe(0);
    tracker.applyScrollDelta(600, 1500, 500);
    expect(tracker.headerOffset).toBe(-48);

    tracker.correctForScrollPosition(0);
    expect(tracker.headerOffset).toBe(0);
  });

  it('does not force header visible when user is scrolled past header height', () => {
    tracker.switchContainer(0);

    tracker.applyScrollDelta(0, 2000, 500);
    tracker.applyScrollDelta(200, 2000, 500);
    expect(tracker.headerOffset).toBe(-48);

    tracker.correctForScrollPosition(200);
    expect(tracker.headerOffset).toBe(-48);
  });

  it('shows the header once a shrink brings the scroll within its height', () => {
    tracker.switchContainer(0);

    tracker.applyScrollDelta(0, 1000, 500);
    tracker.applyScrollDelta(100, 1000, 500);
    expect(tracker.headerOffset).toBe(-48);

    tracker.correctForScrollPosition(20);
    expect(tracker.headerOffset).toBe(0);
  });
});

describe('useHideOnScroll title bar height var', () => {
  function computeTitleBarHeightVar(heightPx: number, remSize: number): string | null {
    if (heightPx <= 0) return null;
    return `${heightPx / remSize}rem`;
  }

  it('single-line title (44px) converts to rem at 16px base', () => {
    expect(computeTitleBarHeightVar(44, 16)).toBe(`${44 / 16}rem`);
  });

  it('multiline title (80px) converts to rem — chevron must track wrapped height', () => {
    expect(computeTitleBarHeightVar(80, 16)).toBe(`${80 / 16}rem`);
  });

  it('mobile rem base (18px) — same px height yields smaller rem value', () => {
    expect(computeTitleBarHeightVar(80, 18)).toBe(`${80 / 18}rem`);
  });

  it('returns null when title bar is absent (chevron uses fallback 0)', () => {
    expect(computeTitleBarHeightVar(0, 16)).toBe(null);
  });
});

describe('useHideOnScroll iOS keyboard VV offset', () => {
  function computeHeaderTop(vvOffsetTop: number, remSize: number): string {
    return vvOffsetTop > 0 ? `${vvOffsetTop / remSize}rem` : '';
  }

  it('adjusts header top when iOS scrolls visual viewport down', () => {
    expect(computeHeaderTop(50, 18)).toBe(`${50 / 18}rem`);
  });

  it('no adjustment when visual viewport is at top', () => {
    expect(computeHeaderTop(0, 18)).toBe('');
  });

  it('resets to empty string (no inline style) when keyboard closes', () => {
    expect(computeHeaderTop(0, 16)).toBe('');
  });
});

describe('useHideOnScroll stale keyboard recovery', () => {
  let mockActive: { tagName: string; pane?: string } | null = null;
  let tracker: ReturnType<typeof createScrollTracker>;

  beforeEach(() => {
    mockActive = null;
    tracker = createScrollTracker(() => mockActive);
  });

  it('header stays permanently hidden when keyboardOpen is stale (iOS swipe bug)', () => {
    tracker.switchContainer(0);

    tracker.setKeyboardOpen(true);
    tracker.onFocusIn(0);
    expect(tracker.getEffectiveOffset()).toBe(-48);

    tracker.switchContainer(0);

    expect(tracker.keyboardOpen).toBe(true);
    expect(tracker.getEffectiveOffset()).toBe(-48);

    tracker.applyScrollDelta(0, 1000, 500);
    tracker.applyScrollDelta(50, 1000, 500);
    tracker.applyScrollDelta(20, 1000, 500);
    expect(tracker.headerOffset).toBe(0);
    expect(tracker.getEffectiveOffset()).toBe(-48);

    tracker.recoverKeyboardState(false);
    expect(tracker.keyboardOpen).toBe(false);
    expect(tracker.getEffectiveOffset()).toBe(0);
  });

  it('does not reset keyboardOpen when text input is still focused', () => {
    tracker.switchContainer(0);
    tracker.setKeyboardOpen(true);
    tracker.onFocusIn(0);

    tracker.recoverKeyboardState(true);
    expect(tracker.keyboardOpen).toBe(true);
    expect(tracker.getEffectiveOffset()).toBe(-48);
  });

  it('recovers header on pane switch when focusout was missed', () => {
    tracker.switchContainer(0);
    tracker.setKeyboardOpen(true);
    tracker.onFocusIn(0);

    tracker.switchContainer(0);
    tracker.recoverKeyboardState(false);

    expect(tracker.keyboardOpen).toBe(false);
    expect(tracker.headerOffset).toBe(0);
    expect(tracker.getEffectiveOffset()).toBe(0);
  });
});

describe('useHideOnScroll app UI disabled mode', () => {
  let mockActive: { tagName: string; pane?: string } | null = null;
  let tracker: ReturnType<typeof createScrollTracker>;

  beforeEach(() => {
    mockActive = null;
    tracker = createScrollTracker(() => mockActive);
  });

  it('returns 0 (fully visible) when disabled, even if header was hidden by scroll', () => {
    tracker.switchContainer(0);

    tracker.applyScrollDelta(0, 1000, 500);
    tracker.applyScrollDelta(100, 1000, 500);
    expect(tracker.headerOffset).toBe(-48);
    expect(tracker.getEffectiveOffset()).toBe(-48);

    tracker.setDisabled(true);
    expect(tracker.getEffectiveOffset()).toBe(0);
  });

  it('ignores scroll events while disabled (effective offset stays 0)', () => {
    tracker.switchContainer(0);
    tracker.setDisabled(true);

    tracker.applyScrollDelta(0, 1000, 500);
    tracker.applyScrollDelta(50, 1000, 500);
    tracker.applyScrollDelta(100, 1000, 500);

    expect(tracker.getEffectiveOffset()).toBe(0);
  });

  it('picks the scroll back up where it was when disabled is cleared', () => {
    tracker.switchContainer(0);

    tracker.setDisabled(true);
    tracker.applyScrollDelta(0, 1000, 500);
    tracker.applyScrollDelta(100, 1000, 500);
    expect(tracker.getEffectiveOffset()).toBe(0);

    tracker.setDisabled(false);
    expect(tracker.getEffectiveOffset()).toBe(0);
    tracker.applyScrollDelta(200, 1000, 500);
    expect(tracker.getEffectiveOffset()).toBe(-48);
  });

  it('disabled takes priority over keyboard state', () => {
    tracker.switchContainer(0);
    tracker.setKeyboardOpen(true);
    tracker.setDisabled(true);

    expect(tracker.getEffectiveOffset()).toBe(0);
  });

  it('clears stale keyboard state when entering disabled mode', () => {
    tracker.switchContainer(0);

    tracker.setKeyboardOpen(true);
    expect(tracker.keyboardOpen).toBe(true);

    tracker.setDisabled(true);
    expect(tracker.getEffectiveOffset()).toBe(0);
  });
});

function simulateFocusCompensation(opts: {
  cachedHeight: number;
  controlsTop: number;
  scrollTop: number;
  firstInputContentY: number;
}) {
  const { cachedHeight, controlsTop, scrollTop, firstInputContentY } = opts;
  const delta = cachedHeight - controlsTop;
  const newScrollTop = Math.max(0, scrollTop - delta);
  const newContentY = firstInputContentY - delta;
  const layoutY = newContentY - newScrollTop;
  return { newScrollTop, layoutY };
}

describe('useHideOnScroll keyboard open compensation respects safe-area', () => {
  const cachedHeight = 94;
  const controlsTop = 50;

  it('first input (at top of scroll) lands at safe-area-top, not behind dynamic island', () => {
    const { newScrollTop, layoutY } = simulateFocusCompensation({
      cachedHeight,
      controlsTop,
      scrollTop: 0,
      firstInputContentY: cachedHeight,
    });

    expect(newScrollTop).toBe(0);
    expect(layoutY).toBe(controlsTop);
  });

  it('scrolled-down input stays anchored to its pre-focus layout position', () => {
    const scrollTop = 500;
    const inputContentY = 600;

    const { newScrollTop, layoutY } = simulateFocusCompensation({
      cachedHeight,
      controlsTop,
      scrollTop,
      firstInputContentY: inputContentY,
    });

    expect(newScrollTop).toBe(scrollTop - (cachedHeight - controlsTop));
    expect(layoutY).toBe(inputContentY - scrollTop);
  });

  it('no-op on platforms without a safe area (controlsTop = 0)', () => {
    const { newScrollTop, layoutY } = simulateFocusCompensation({
      cachedHeight,
      controlsTop: 0,
      scrollTop: 0,
      firstInputContentY: cachedHeight,
    });

    expect(newScrollTop).toBe(0);
    expect(layoutY).toBe(0);
  });
});

function createTransformTracker() {
  const cachedHeight = 48;
  let keyboardOpen = false;
  let away = true;
  let appliedOffset: number | null = null;

  function applyTransform() {
    appliedOffset = keyboardOpen ? -cachedHeight : away ? -cachedHeight : 0;
  }

  function syncToScroll() {
    away = false;
    applyTransform();
  }

  function onFocusIn() {
    keyboardOpen = true;
    applyTransform();
  }

  function recoverKeyboardState(isInputFocused: boolean) {
    if (!keyboardOpen) return;
    if (isInputFocused) return;
    keyboardOpen = false;
    syncToScroll();
  }

  return {
    get appliedOffset() { return appliedOffset; },
    get keyboardOpen() { return keyboardOpen; },
    onFocusIn,
    recoverKeyboardState,
  };
}

describe('shouldKeepHeaderVisible', () => {
  it('lets the bars move when dynamic bars are on and no find bar is open, an app included', () => {
    expect(shouldKeepHeaderVisible({ dynamicBars: true, findOpen: false })).toBe(false);
  });

  it('pins the bars when dynamic bars are off', () => {
    expect(shouldKeepHeaderVisible({ dynamicBars: false, findOpen: false })).toBe(true);
  });

  it('pins the bars while a find bar is open', () => {
    expect(shouldKeepHeaderVisible({ dynamicBars: true, findOpen: true })).toBe(true);
  });
});

describe('spacerHeightPx', () => {
  const cachedHeight = 94;
  const controlsTop = 50;

  it('collapses to safe-area-top when keyboard opens and header is NOT pinned', () => {
    expect(spacerHeightPx({ cachedHeight, controlsTop, keyboardOpen: true, disabled: false }))
      .toBe(controlsTop);
  });

  it('stays full height when header is pinned, even with keyboard open', () => {
    expect(spacerHeightPx({ cachedHeight, controlsTop, keyboardOpen: true, disabled: true }))
      .toBe(cachedHeight);
  });

  it('stays full height when keyboard is closed', () => {
    expect(spacerHeightPx({ cachedHeight, controlsTop, keyboardOpen: false, disabled: false }))
      .toBe(cachedHeight);
    expect(spacerHeightPx({ cachedHeight, controlsTop, keyboardOpen: false, disabled: true }))
      .toBe(cachedHeight);
  });
});

describe('useHideOnScroll recovery re-applies header transform', () => {
  it('restores the header when iOS missed the focusout', () => {
    const t = createTransformTracker();

    t.onFocusIn();
    expect(t.appliedOffset).toBe(-48);

    t.recoverKeyboardState(false);

    expect(t.keyboardOpen).toBe(false);
    expect(t.appliedOffset).toBe(0);
  });

  it('shows the header after recovery, even scrolled far down', () => {
    const t = createTransformTracker();

    t.onFocusIn();
    t.recoverKeyboardState(false);

    expect(t.appliedOffset).toBe(0);
  });

  it('leaves transform untouched when an input is still focused', () => {
    const t = createTransformTracker();

    t.onFocusIn();
    const before = t.appliedOffset;
    t.recoverKeyboardState(true);

    expect(t.keyboardOpen).toBe(true);
    expect(t.appliedOffset).toBe(before);
  });
});

describe('useHideOnScroll across an anchored reveal correction', () => {
  let anchoring = false;
  let navigating = false;

  beforeEach(() => {
    anchoring = false;
    navigating = false;
  });

  function tracker() {
    return createScrollTracker(
      () => null,
      () => navigating,
      () => false,
      () => false,
      () => anchoring,
    );
  }

  it('keeps the header hidden when the correction scrolls the reader up', () => {
    const t = tracker();
    t.switchContainer(0);
    t.applyScrollDelta(3000, 12000, 800);
    expect(t.headerOffset).toBe(-48);

    navigating = true;
    anchoring = true;
    t.applyScrollDelta(2060, 8000, 800);

    expect(t.headerOffset).toBe(-48);
  });

  it('still reveals the header for a navigation that is not an anchor', () => {
    const t = tracker();
    t.switchContainer(0);
    t.applyScrollDelta(3000, 12000, 800);
    expect(t.headerOffset).toBe(-48);

    navigating = true;
    t.applyScrollDelta(2060, 12000, 800);

    expect(t.headerOffset).toBe(0);
  });

  it('spends nothing when the correction has already re-based the baseline', () => {
    const t = tracker();
    t.switchContainer(0);
    t.applyScrollDelta(3000, 12000, 800);
    expect(t.headerOffset).toBe(-48);

    t.rebase(2060, 8000, 800);
    t.applyScrollDelta(2060, 8000, 800);

    expect(t.headerOffset).toBe(-48);
  });

  it('reads the reader own scroll after the re-base from the new baseline', () => {
    const t = tracker();
    t.switchContainer(0);
    t.applyScrollDelta(3000, 12000, 800);

    t.rebase(2060, 8000, 800);
    t.applyScrollDelta(2066, 8000, 800);
    expect(t.headerOffset).toBe(-48);

    t.applyScrollDelta(2030, 8000, 800);
    expect(t.headerOffset).toBe(0);
  });

  it('keeps the header hidden when a later mark has stolen the anchor kind', () => {
    const t = tracker();
    t.switchContainer(0);
    t.applyScrollDelta(3000, 12000, 800);
    expect(t.headerOffset).toBe(-48);

    t.rebase(2060, 8000, 800);
    navigating = true;
    anchoring = false;
    t.applyScrollDelta(2060, 8000, 800);

    expect(t.headerOffset).toBe(-48);
  });

  it('spends the 1px repaint nudge without reading it as the reader moving', () => {
    const t = tracker();
    t.switchContainer(0);
    t.applyScrollDelta(3000, 12000, 800);

    t.rebase(2060, 8000, 800);
    navigating = true;
    t.applyScrollDelta(2059, 8000, 800);

    expect(t.headerOffset).toBe(-48);
  });

  it('still reveals for a navigation that lands somewhere else', () => {
    const t = tracker();
    t.switchContainer(0);
    t.applyScrollDelta(3000, 12000, 800);

    t.rebase(2060, 8000, 800);
    navigating = true;
    t.applyScrollDelta(400, 8000, 800);

    expect(t.headerOffset).toBe(0);
  });

  it('spends the stamp once the reader scrolls off the anchored position', () => {
    const t = tracker();
    t.switchContainer(0);
    t.applyScrollDelta(3000, 12000, 800);

    t.rebase(2060, 8000, 800);
    t.applyScrollDelta(2400, 8000, 800);
    navigating = true;
    t.applyScrollDelta(2060, 8000, 800);

    expect(t.headerOffset).toBe(0);
  });
});
