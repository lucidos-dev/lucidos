import { describe, it, expect, beforeAll } from 'vitest';
import { installNoKeyCodeText } from './noKeyCodeText';

// The test env is `node`, so `document` is the stub in `src/test-setup.ts`: it
// keeps a listener list and dispatches a plain object to it. That pins the
// wiring: the event name the guard listens on, and the cancel it answers with.
// The rule itself is tested in the SDK's `textEntry.test.ts`.
describe('installNoKeyCodeText', () => {
  beforeAll(() => {
    installNoKeyCodeText();
  });

  function type(data: string): boolean {
    let prevented = false;
    document.dispatchEvent({
      type: 'beforeinput',
      inputType: 'insertText',
      data,
      preventDefault: () => { prevented = true; },
    } as unknown as Event);
    return prevented;
  }

  it('cancels the right arrow typed at the end of the prompt', () => {
    expect(type(String.fromCodePoint(0x1d))).toBe(true);
  });

  it('cancels the left arrow typed at the start of the prompt', () => {
    expect(type(String.fromCodePoint(0x1c))).toBe(true);
  });

  it('lets ordinary typing through', () => {
    expect(type('a')).toBe(false);
  });
});
