import { describe, it, expect } from 'vitest';
import { filterSlashCommands, parseSlashFilter, slashCommandText } from './slashCommandFilter';

const COMMANDS = ['codex:adversarial-review', 'codex:review', 'codex:status', 'compact'];

describe('parseSlashFilter', () => {
  it('reads a bare name as the query, with no arguments', () => {
    expect(parseSlashFilter('Review')).toEqual({ query: 'review', args: '', nameEnded: false });
  });

  it('splits the name from its arguments at the first space', () => {
    expect(parseSlashFilter('codex:review --model gpt-5.6-sol --wait')).toEqual({
      query: 'codex:review',
      args: '--model gpt-5.6-sol --wait',
      nameEnded: true,
    });
  });

  it('ends the name on a trailing space', () => {
    expect(parseSlashFilter('codex:review ')).toEqual({ query: 'codex:review', args: '', nameEnded: true });
  });
});

describe('filterSlashCommands', () => {
  it('lists every command for an empty filter', () => {
    expect(filterSlashCommands(COMMANDS, '')).toEqual(COMMANDS);
  });

  it('matches part of a name while the name is still being typed', () => {
    expect(filterSlashCommands(COMMANDS, 'review')).toEqual(['codex:adversarial-review', 'codex:review']);
  });

  it('keeps the named command once arguments follow', () => {
    expect(filterSlashCommands(COMMANDS, 'codex:review --wait')).toEqual(['codex:review']);
  });

  it('matches the name exactly once a space ends it', () => {
    expect(filterSlashCommands(COMMANDS, 'review --wait')).toEqual([]);
  });
});

describe('slashCommandText', () => {
  it('sends the bare command when nothing follows the name', () => {
    expect(slashCommandText('codex:review', 'rev')).toBe('/codex:review');
  });

  it('sends the typed arguments with the picked command', () => {
    expect(slashCommandText('codex:review', 'codex:review --model gpt-5.6-sol --wait'))
      .toBe('/codex:review --model gpt-5.6-sol --wait');
  });
});
