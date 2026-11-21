/**
 * Every Settings row that carries a search anchor is findable in Search
 * everywhere. Models → Voice once rendered its anchors with no search entry,
 * so typing "voice" found nothing, and nothing failed.
 *
 * An entry covers an anchor when it lands on it. It also covers it when it
 * takes the anchor as its id and lands on an ancestor that always renders.
 * That is how a row shown only while a switch is on lands on the switch.
 */
import { describe, it, expect } from 'vitest';
import { settingsSearchCovers } from './searchIndex';
import {
  renderedSearchAnchors,
  staleDynamicAnchorRows,
  unclassifiedDynamicAnchors,
} from './__tests__/renderedSearchAnchors';

describe('every Settings search anchor reaches Search everywhere', () => {
  it('gives each rendered anchor a search entry', () => {
    const uncovered = [...renderedSearchAnchors()].filter((a) => !settingsSearchCovers(a)).sort();
    expect(uncovered, 'add a SETTINGS_SEARCH_INDEX entry in searchIndex.ts for each').toEqual([]);
  });

  it('knows where every runtime-built anchor gets its values', () => {
    expect(
      unclassifiedDynamicAnchors(),
      'add a DYNAMIC_ANCHORS row in __tests__/renderedSearchAnchors.ts for each',
    ).toEqual([]);
  });

  it('keeps no row for an expression that is gone', () => {
    expect(staleDynamicAnchorRows()).toEqual([]);
  });
});
