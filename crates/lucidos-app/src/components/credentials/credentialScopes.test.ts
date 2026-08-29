import { describe, it, expect } from 'vitest';
import {
  addScopeRow,
  removeScopeRow,
  seedScopeRows,
  setScopeRow,
  submittedScopes,
} from './credentialScopes';

describe('seedScopeRows', () => {
  it('shows every stored base URL', () => {
    expect(seedScopeRows(['https://api.binance.com', 'https://fapi.binance.com'])).toEqual([
      'https://api.binance.com',
      'https://fapi.binance.com',
    ]);
  });

  it('always leaves one field to type into', () => {
    expect(seedScopeRows(undefined)).toEqual(['']);
    expect(seedScopeRows([])).toEqual(['']);
    expect(seedScopeRows(['  '])).toEqual(['']);
  });
});

describe('editing the rows', () => {
  it('adds, edits and removes without disturbing the others', () => {
    let rows = seedScopeRows(['https://api.binance.com']);
    rows = addScopeRow(rows);
    rows = setScopeRow(rows, 1, 'https://fapi.binance.com');
    expect(rows).toEqual(['https://api.binance.com', 'https://fapi.binance.com']);

    rows = removeScopeRow(rows, 0);
    expect(rows).toEqual(['https://fapi.binance.com']);
  });

  /* Removing the last row must leave a field on screen. Emptying the list
     would strand the user with no input and only an Add button. */
  it('keeps one field when the last row is removed', () => {
    expect(removeScopeRow(['https://api.binance.com'], 0)).toEqual(['']);
  });
});

describe('submittedScopes', () => {
  it('trims, drops blanks and collapses duplicates in order', () => {
    expect(
      submittedScopes([
        '  https://api.binance.com ',
        '',
        'https://fapi.binance.com',
        'https://api.binance.com',
      ]),
    ).toEqual(['https://api.binance.com', 'https://fapi.binance.com']);
  });

  /* The empty scope is reachable and means the credential goes nowhere, which
     is what a `secret` carries. */
  it('submits nothing when every row is blank', () => {
    expect(submittedScopes([''])).toEqual([]);
    expect(submittedScopes(['   ', ''])).toEqual([]);
  });
});
