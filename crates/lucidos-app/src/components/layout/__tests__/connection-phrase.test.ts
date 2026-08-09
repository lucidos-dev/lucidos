import { describe, it, expect } from 'vitest';
import { connectionPhrase } from '../HeaderMark';

/** The mark's readable half. It is the accessible name AND the desktop hover
 *  tooltip, so it has to read as English in all three states and survive the
 *  window before /health has named the workspace. */
describe('connectionPhrase', () => {
  it('names what the mark is connected to, with the preposition each state wants', () => {
    expect(connectionPhrase('connected', 'dev')).toBe('connected to dev');
    expect(connectionPhrase('connecting', 'dev')).toBe('connecting to dev');
    // Not "disconnected TO dev": each state brings its own preposition for
    // exactly this one.
    expect(connectionPhrase('disconnected', 'dev')).toBe('disconnected from dev');
  });

  it('falls back to the bare state before the workspace has a name', () => {
    for (const state of ['connected', 'connecting', 'disconnected']) {
      expect(connectionPhrase(state, null)).toBe(state);
      expect(connectionPhrase(state, '')).toBe(state);
    }
  });

  it('passes an unknown state through rather than inventing a sentence', () => {
    expect(connectionPhrase('reconnecting-soon', 'dev')).toBe('reconnecting-soon');
  });
});
