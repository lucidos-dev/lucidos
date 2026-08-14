import { describe, it, expect, afterEach } from 'vitest';
import {
  CHOICE_CARD_ROLE,
  claimSeedForCard,
  handleChoiceCardKeyDown,
  nextChoiceIndex,
  shouldSeedChoiceFocus,
} from '../choiceCardNav';
import { isNavigationScroll, setActiveScrollElement } from '../scrollState';

const key = (
  k: string,
  mods: Partial<Pick<KeyboardEvent, 'metaKey' | 'ctrlKey' | 'altKey'>> = {},
) => ({ key: k, metaKey: false, ctrlKey: false, altKey: false, ...mods });

describe('CHOICE_CARD_ROLE', () => {
  it('is the literal the CSS and the e2e selectors are written against', () => {
    // Duplicated by hand in `.permission-body[data-role="card-choices"]`
    // (styles/chat/response.css) and in the browser e2e selectors, neither of
    // which can import the constant. Changing it means changing those too.
    expect(CHOICE_CARD_ROLE).toBe('card-choices');
  });
});

describe('nextChoiceIndex', () => {
  it('steps forward on ArrowDown and ArrowRight, backward on ArrowUp and ArrowLeft', () => {
    // Both axes drive the same walk: the question card is a vertical list, the
    // permission card mixes a horizontal primary row with stacked secondary
    // rows, so neither axis alone reads correctly on both.
    expect(nextChoiceIndex(key('ArrowDown'), 1, 4)).toBe(2);
    expect(nextChoiceIndex(key('ArrowRight'), 1, 4)).toBe(2);
    expect(nextChoiceIndex(key('ArrowUp'), 2, 4)).toBe(1);
    expect(nextChoiceIndex(key('ArrowLeft'), 2, 4)).toBe(1);
  });

  it('clamps at both ends instead of wrapping', () => {
    // Matches ThreadDrawer.moveHighlight and Dropdown: an arrow held at the end
    // of the list stays put rather than jumping to the other end.
    expect(nextChoiceIndex(key('ArrowUp'), 0, 3)).toBe(0);
    expect(nextChoiceIndex(key('ArrowDown'), 2, 3)).toBe(2);
  });

  it('seeds from "focus is on no choice" to the first going forward, the last going backward', () => {
    expect(nextChoiceIndex(key('ArrowDown'), -1, 3)).toBe(0);
    expect(nextChoiceIndex(key('ArrowUp'), -1, 3)).toBe(2);
  });

  it('declines every key that is not an arrow', () => {
    // Enter and Space must reach the button's own native activation, and a
    // printable key must fall through to type-to-focus-prompt (the free-text
    // escape the question card's hint promises).
    for (const k of ['Enter', ' ', 'a', 'Escape', 'Tab', 'Home']) {
      expect(nextChoiceIndex(key(k), 0, 3)).toBeNull();
    }
  });

  it('declines any chord carrying a primary modifier or Alt', () => {
    // Those are global shortcuts (turn nav, history, maximize pane group) and
    // must bubble to the document handler untouched. Same guard as
    // ThreadDrawer.handleKeyDown.
    expect(nextChoiceIndex(key('ArrowDown', { metaKey: true }), 0, 3)).toBeNull();
    expect(nextChoiceIndex(key('ArrowDown', { ctrlKey: true }), 0, 3)).toBeNull();
    expect(nextChoiceIndex(key('ArrowUp', { altKey: true }), 0, 3)).toBeNull();
  });

  it('declines when the card has no enabled choices', () => {
    // A card mid-resolution has every button disabled; arrows must not consume
    // the keystroke there.
    expect(nextChoiceIndex(key('ArrowDown'), -1, 0)).toBeNull();
  });
});

describe('claimSeedForCard', () => {
  it('grants the seed once per card id and never again', () => {
    // The seed belongs to a card's ARRIVAL, and `live` is not a one-way flip:
    // answering is optimistic, so a failed send rolls it back and the card
    // returns to live. Without the latch that rollback re-seeds the DEFAULT
    // choice, which on a permission card drags focus to "Allow once" moments
    // after the user pressed Deny and the send failed. The visible ring is no
    // defence there: the user just expressed the opposite intent and has no
    // reason to re-read the card.
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
  const idle = { hoverPointer: true, promptHasText: false, activeIsIdle: true };

  it('seeds when the user is idle at the bottom of the transcript on desktop', () => {
    expect(shouldSeedChoiceFocus(idle)).toBe(true);
  });

  it('never seeds without a hover-capable pointer', () => {
    // The gate is hasHoverPointer(), NOT isMobile(): the question is whether a
    // keyboard exists to press the seeded choice with. An iPad in landscape is
    // wider than the mobile breakpoint and still has none, so a width test would
    // programmatically focus a button there and strand a stray ring. This is
    // also the JS mirror of the `@media (hover: hover)` gate on the ring itself,
    // so styling and focus behaviour cannot drift apart.
    expect(shouldSeedChoiceFocus({ ...idle, hoverPointer: false })).toBe(false);
  });

  it('never seeds while the prompt holds text', () => {
    // The user is composing a free-text answer; taking the caret would drop
    // their in-flight keystrokes.
    expect(shouldSeedChoiceFocus({ ...idle, promptHasText: true })).toBe(false);
  });

  it('never seeds while focus is on some other control', () => {
    // An app iframe, a settings field, a drawer row: theirs, not ours.
    expect(shouldSeedChoiceFocus({ ...idle, activeIsIdle: false })).toBe(false);
  });

  it('asks nothing about the transcript position', () => {
    // There used to be a fourth clause here, a position SIGNAL standing for
    // "the reader has chosen to read history". It went with the bottom-pin that
    // maintained it, and it is deliberately NOT carried over to
    // `awayFromBottom`: nothing scrolls to a card now, so the card's own
    // arrival puts the reader off the bottom and every card in a scrollable
    // thread would decline its one chance at focus (`claimSeedForCard` latches
    // the id on arrival). `seedChoiceCardFocus` asks `isElementOnScreen` about
    // the CHOICE instead, which is the question that actually matters.
    expect(Object.keys(idle)).toEqual(['hoverPointer', 'promptHasText', 'activeIsIdle']);
  });
});

describe('the arrow-key reveal announces itself as the app\'s own', () => {
  afterEach(() => { setActiveScrollElement(null); });

  /** The transcript, and a card holding two choices inside it. Only the pieces
   *  `handleChoiceCardKeyDown` touches: the buttons it walks, the focus and
   *  reveal it calls on them, and enough box for `isElementVisible`. */
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
    // `scrollIntoView` moves the transcript without writing `scrollTop`, and the
    // keydown lands on the BUTTON rather than on the transcript, so neither of
    // the two signals that tell the app's scrolls from the reader's sees it. The
    // mobile header slid away under an arrow step because of that, the render
    // window read it as a request for older turns, and the *standing follow*'s
    // platform-scroll correction wrote an armed reader back to the live edge,
    // taking the option they had just stepped to off the screen. Found by the
    // second hardening reviewer, 2026-08-13.
    const { transcript, root } = cardInATranscript();
    const origActive = (globalThis.document as any).activeElement;
    (globalThis.document as any).activeElement = null;   // focus on no choice: the arrow seeds
    try {
      expect(isNavigationScroll(transcript)).toBe(false);

      handleChoiceCardKeyDown(
        { key: 'ArrowDown', metaKey: false, ctrlKey: false, altKey: false, preventDefault() {} } as any,
        root,
      );

      expect(transcript.scrollTop).toBe(900);            // the reveal happened
      expect(isNavigationScroll(transcript)).toBe(true); // and it is ours
    } finally {
      (globalThis.document as any).activeElement = origActive;
    }
  });
});
