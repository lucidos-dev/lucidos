import { describe, it, expect } from 'vitest';
import {
  addScopeRow,
  initialScopeRows,
  removeScopeRow,
  setScopeRow,
  submittedScopes,
} from './credentialScopes';

describe('initialScopeRows', () => {
  it('shows every stored base URL', () => {
    expect(
      initialScopeRows(undefined, ['https://api.binance.com', 'https://fapi.binance.com']),
    ).toEqual(['https://api.binance.com', 'https://fapi.binance.com']);
  });

  it('always leaves one field to type into', () => {
    expect(initialScopeRows(undefined, undefined)).toEqual(['']);
    expect(initialScopeRows([], [])).toEqual(['']);
    expect(initialScopeRows(undefined, ['  '])).toEqual(['']);
  });

  it("takes the request's set over the stored one, because the engine unioned it", () => {
    expect(
      initialScopeRows(
        ['https://api.github.com', 'https://github.com'],
        ['https://api.github.com'],
      ),
    ).toEqual(['https://api.github.com', 'https://github.com']);
  });

  it('falls back to the stored set when the request names none', () => {
    expect(initialScopeRows(undefined, ['https://api.dropboxapi.com'])).toEqual([
      'https://api.dropboxapi.com',
    ]);
    expect(initialScopeRows([], ['https://api.dropboxapi.com'])).toEqual([
      'https://api.dropboxapi.com',
    ]);
  });

});

describe('editing the rows', () => {
  it('adds, edits and removes without disturbing the others', () => {
    let rows = initialScopeRows(undefined, ['https://api.binance.com']);
    rows = addScopeRow(rows);
    rows = setScopeRow(rows, 1, 'https://fapi.binance.com');
    expect(rows).toEqual(['https://api.binance.com', 'https://fapi.binance.com']);

    rows = removeScopeRow(rows, 0);
    expect(rows).toEqual(['https://fapi.binance.com']);
  });

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

  it('submits nothing when every row is blank', () => {
    expect(submittedScopes([''])).toEqual([]);
    expect(submittedScopes(['   ', ''])).toEqual([]);
  });
});
