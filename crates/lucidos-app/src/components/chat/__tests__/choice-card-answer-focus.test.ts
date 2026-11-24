// @vitest-environment jsdom
import { describe, it, expect, afterEach, beforeEach } from 'vitest';
import { focusedPane } from '../../../store/store';
import { CHOICE_CARD_ROLE, handAnsweredCardFocusToPrompt } from '../choiceCardNav';

describe('handAnsweredCardFocusToPrompt', () => {
  beforeEach(() => {
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
    threadPane();
    const elsewhere = document.querySelector<HTMLButtonElement>('.thread-drawer button')!;
    elsewhere.focus();

    handAnsweredCardFocusToPrompt();

    expect(document.activeElement).toBe(elsewhere);
  });
});
