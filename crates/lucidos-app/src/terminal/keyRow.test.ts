import { describe, it, expect } from 'vitest';
import { CTRL_KEY_ID, KEY_ROW, PASTE_KEY_ID, withControl } from './keyRow';

const bytesOf = (id: string) => KEY_ROW.find((key) => key.id === id)?.bytes;

describe('the phone key row', () => {
  it('sends Escape and the arrow sequences a shell reads', () => {
    expect(bytesOf('esc')).toBe('\x1b');
    expect(bytesOf('tab')).toBe('\t');
    expect(bytesOf('up')).toBe('\x1b[A');
    expect(bytesOf('left')).toBe('\x1b[D');
  });

  it('holds Ctrl and Paste as actions, not bytes', () => {
    expect(bytesOf(CTRL_KEY_ID)).toBeUndefined();
    expect(bytesOf(PASTE_KEY_ID)).toBeUndefined();
  });

  it('labels every key for a screen reader', () => {
    expect(KEY_ROW.every((key) => key.ariaLabel.length > 0)).toBe(true);
  });
});

describe('a sticky Ctrl', () => {
  it('turns a letter into its control code, in either case', () => {
    expect(withControl('c')).toBe('\x03');
    expect(withControl('C')).toBe('\x03');
    expect(withControl('d')).toBe('\x04');
    expect(withControl('[')).toBe('\x1b');
  });

  it('leaves text it has no control code for alone', () => {
    expect(withControl('1')).toBe('1');
    expect(withControl('ls')).toBe('ls');
  });
});
