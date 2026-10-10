// @vitest-environment jsdom
import { describe, it, expect, afterEach } from 'vitest';
import { focusedPane, splitRatio } from '../store';
import { holdFocusedPaneWhileTyping, releaseTypingHold, revealContentPane } from './pane';

// An agent opens a file while the reader types in the prompt. The reveal's
// reconcile never pulls focus off a field being edited. Moving the marker
// alone would send their next Tab into the content pane instead of to Send.
describe('revealContentPane while the reader types in another pane', () => {
  afterEach(() => {
    document.body.innerHTML = '';
    focusedPane.value = 'thread';
  });

  function panes(): { prompt: HTMLTextAreaElement; button: HTMLButtonElement } {
    document.body.innerHTML = `
      <div class="pane-thread"><textarea></textarea><button>Send</button></div>
      <div class="pane-content"></div>`;
    splitRatio.value = 0.5;
    return { prompt: document.querySelector('textarea')!, button: document.querySelector('button')! };
  }

  it('an agent navigation keeps the marker with the text field', () => {
    panes().prompt.focus();
    holdFocusedPaneWhileTyping();
    revealContentPane();
    expect(focusedPane.value).toBe('thread');
  });

  it('the reader\'s own navigation from the prompt still moves the marker', () => {
    // A shortcut such as the one that opens Settings: Tab should follow it.
    panes().prompt.focus();
    revealContentPane();
    expect(focusedPane.value).toBe('content');
  });

  it('an agent navigation still moves the marker when focus is on a plain control', () => {
    panes().button.focus();
    holdFocusedPaneWhileTyping();
    revealContentPane();
    expect(focusedPane.value).toBe('content');
  });

  it('a released hold does not catch the reader\'s later reveal', () => {
    // An agent navigation that landed no content, such as a thread or a URL.
    panes().prompt.focus();
    holdFocusedPaneWhileTyping();
    releaseTypingHold();
    revealContentPane();
    expect(focusedPane.value).toBe('content');
  });

  it('a hold is spent by one reveal', () => {
    panes().prompt.focus();
    holdFocusedPaneWhileTyping();
    revealContentPane();
    focusedPane.value = 'thread';
    revealContentPane();
    expect(focusedPane.value).toBe('content');
  });
});
