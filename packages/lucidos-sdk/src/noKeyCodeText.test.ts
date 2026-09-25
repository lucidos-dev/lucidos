// @vitest-environment jsdom
import { describe, it, expect, beforeAll } from 'vitest';
import { installNoKeyCodeText } from './noKeyCodeText';

// Pins the wiring: the event the guard listens on, and the cancel it answers
// with. The rule itself is tested in `textEntry.test.ts`.
describe('installNoKeyCodeText', () => {
  beforeAll(() => {
    installNoKeyCodeText();
  });

  function type(data: string): boolean {
    const field = document.createElement('textarea');
    document.body.appendChild(field);
    const event = new InputEvent('beforeinput', {
      inputType: 'insertText', data, bubbles: true, cancelable: true,
    });
    field.dispatchEvent(event);
    field.remove();
    return event.defaultPrevented;
  }

  it('cancels the right arrow typed at the end of a field', () => {
    expect(type(String.fromCodePoint(0x1d))).toBe(true);
  });

  it('lets ordinary typing through', () => {
    expect(type('a')).toBe(false);
  });
});
