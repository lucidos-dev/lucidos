// @vitest-environment jsdom
import { describe, it, expect, afterEach, beforeEach } from 'vitest';
import { focusedPane } from '../../../store/store';
import { CHOICE_CARD_ROLE, handAnsweredCardFocusToPrompt } from '../choiceCardNav';

// Answering a card swaps its live choices for the answered body. The focused
// option unmounts, focus drops to <body>, and the next Tab restarts at the
// thread title. The answer hands focus to the prompt instead.
describe('handAnsweredCardFocusToPrompt', () => {
  beforeEach(() => {
    // A keyboard device: the gate that keeps a phone's keyboard down.
    window.matchMedia = ((q: string) => ({ matches: q === '(hover: hover)' })) as unknown as typeof window.matchMedia;
  });
  afterEach(() => {
    document.body.innerHTML = '';
    focusedPane.value = 'thread';
  });

  function threadPane(): { option: HTMLButtonElement; prompt: HTMLTextAreaElement } {
    document.body.innerHTML = `
      <div class="thread-drawer"><button>See all</button></div>
      <div class="pane-thread">
        <div data-role="${CHOICE_CARD_ROLE}"><button data-id="option">Yes</button></div>
        <textarea data-role="prompt-input"></textarea>
      </div>`;
    return {
      option: document.querySelector('[data-id="option"]')!,
      prompt: document.querySelector('[data-role="prompt-input"]')!,
    };
  }

  it('moves focus from the answered option to the prompt and marks the thread pane', () => {
    const { option, prompt } = threadPane();
    option.focus();
    focusedPane.value = 'drawer';

    handAnsweredCardFocusToPrompt();

    expect(document.activeElement).toBe(prompt);
    expect(focusedPane.value).toBe('thread');
  });

  it('leaves focus alone when the card did not hold it', () => {
    // A tap in Safari answers without focusing the button; the reader's focus
    // is somewhere of their own choosing.
    threadPane();
    const elsewhere = document.querySelector<HTMLButtonElement>('.thread-drawer button')!;
    elsewhere.focus();

    handAnsweredCardFocusToPrompt();

    expect(document.activeElement).toBe(elsewhere);
  });
});
