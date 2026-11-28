import { describe, it, expect, afterEach } from 'vitest';
import {
  CHOICE_CARD_ROLE,
  claimSeedForCard,
  handleChoiceCardKeyDown,
  nextChoiceIndex,
  shouldSeedChoiceFocus,
} from '../choiceCardNav';
import { classifyScroll, setActiveScrollElement } from '../scrollState';

const key = (
  k: string,
  mods: Partial<Pick<KeyboardEvent, 'metaKey' | 'ctrlKey' | 'altKey'>> = {},
) => ({ key: k, metaKey: false, ctrlKey: false, altKey: false, ...mods });

describe('CHOICE_CARD_ROLE', () => {
  it('is the literal the CSS and the e2e selectors are written against', () => {
    expect(CHOICE_CARD_ROLE).toBe('card-choices');
  });
});

describe('nextChoiceIndex', () => {
  it('steps forward on ArrowDown and ArrowRight, backward on ArrowUp and ArrowLeft', () => {
    expect(nextChoiceIndex(key('ArrowDown'), 1, 4)).toBe(2);
    expect(nextChoiceIndex(key('ArrowRight'), 1, 4)).toBe(2);
    expect(nextChoiceIndex(key('ArrowUp'), 2, 4)).toBe(1);
    expect(nextChoiceIndex(key('ArrowLeft'), 2, 4)).toBe(1);
  });

  it('clamps at both ends instead of wrapping', () => {
    expect(nextChoiceIndex(key('ArrowUp'), 0, 3)).toBe(0);
    expect(nextChoiceIndex(key('ArrowDown'), 2, 3)).toBe(2);
  });

  it('seeds from "focus is on no choice" to the first going forward, the last going backward', () => {
    expect(nextChoiceIndex(key('ArrowDown'), -1, 3)).toBe(0);
    expect(nextChoiceIndex(key('ArrowUp'), -1, 3)).toBe(2);
  });

  it('declines every key that is not an arrow', () => {
    for (const k of ['Enter', ' ', 'a', 'Escape', 'Tab', 'Home']) {
      expect(nextChoiceIndex(key(k), 0, 3)).toBeNull();
    }
  });

  it('declines any chord carrying a primary modifier or Alt', () => {
    expect(nextChoiceIndex(key('ArrowDown', { metaKey: true }), 0, 3)).toBeNull();
    expect(nextChoiceIndex(key('ArrowDown', { ctrlKey: true }), 0, 3)).toBeNull();
    expect(nextChoiceIndex(key('ArrowUp', { altKey: true }), 0, 3)).toBeNull();
  });

  it('declines when the card has no enabled choices', () => {
    expect(nextChoiceIndex(key('ArrowDown'), -1, 0)).toBeNull();
  });
});

describe('claimSeedForCard', () => {
  it('grants the seed once per card id and never again', () => {
    expect(claimSeedForCard('tu-arrival')).toBe(true);
    expect(claimSeedForCard('tu-arrival')).toBe(false);
    expect(claimSeedForCard('tu-arrival')).toBe(false);
  });

  it('tracks each card independently', () => {
    expect(claimSeedForCard('req-a')).toBe(true);
    expect(claimSeedForCard('req-b')).toBe(true);
    expect(claimSeedForCard('req-a')).toBe(false);
  });
});

describe('shouldSeedChoiceFocus', () => {
  const idle = { hoverPointer: true, promptHasText: false, activeIsIdle: true, markerOnThreadPane: true };

  it('seeds when the user is idle at the bottom of the transcript on desktop', () => {
    expect(shouldSeedChoiceFocus(idle)).toBe(true);
  });

  it('never seeds without a hover-capable pointer', () => {
    expect(shouldSeedChoiceFocus({ ...idle, hoverPointer: false })).toBe(false);
  });

  it('never seeds while the prompt holds text', () => {
    expect(shouldSeedChoiceFocus({ ...idle, promptHasText: true })).toBe(false);
  });

  it('never seeds while focus is on some other control', () => {
    expect(shouldSeedChoiceFocus({ ...idle, activeIsIdle: false })).toBe(false);
  });

  it('never seeds while the focused-pane marker points at another pane', () => {
    expect(shouldSeedChoiceFocus({ ...idle, markerOnThreadPane: false })).toBe(false);
  });

  it('asks nothing about the transcript position', () => {
    expect(Object.keys(idle)).toEqual(['hoverPointer', 'promptHasText', 'activeIsIdle', 'markerOnThreadPane']);
  });
});

describe('the arrow-key reveal announces itself as the app\'s own', () => {
  afterEach(() => { setActiveScrollElement(null); });

  function cardInATranscript() {
    const transcript: any = {
      parentElement: null,
      scrollTop: 400,
      getBoundingClientRect: () => ({ width: 400, height: 800, top: 0, bottom: 800, left: 0, right: 400 }),
    };
    const buttons = [0, 1].map(() => ({
      focus() {},
      scrollIntoView() { transcript.scrollTop = 900; },
      hasAttribute: () => false,
    }));
    const root: any = { querySelectorAll: () => buttons };
    setActiveScrollElement(transcript);
    return { transcript, root, buttons };
  }

  it('marks the transcript, so nothing reads the reveal as the reader scrolling', () => {
    const { transcript, root } = cardInATranscript();
    const origActive = (globalThis.document as any).activeElement;
    (globalThis.document as any).activeElement = null;
    try {
      expect(classifyScroll(transcript).navigation).toBe(false);

      handleChoiceCardKeyDown(
        { key: 'ArrowDown', metaKey: false, ctrlKey: false, altKey: false, preventDefault() {} } as any,
        root,
      );

      expect(transcript.scrollTop).toBe(900);
      expect(classifyScroll(transcript).navigation).toBe(true);
    } finally {
      (globalThis.document as any).activeElement = origActive;
    }
  });
});
