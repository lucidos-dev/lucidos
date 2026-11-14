/**
 * A find a navigation asks for, before any find bar exists to take it.
 *
 * Kept apart from `find-bar.ts` because the navigation router is in the entry
 * chunk. Importing the bar from there pulls its targets, the transcript find
 * and the markdown renderer into first paint. Keep this module light.
 */

import { findRequest } from '../store';

/** A single letter matches most of a page, and a phone froze painting them. */
export const MIN_FIND_QUERY_CHARS = 2;

/** The length of `query` as the bar counts it: trimmed, in code points. */
export function findQueryChars(query: string): number {
  return [...query.trim()].length;
}

/** The scope of the content pane's bar over a previewed file. */
export function fileFindScope(path: string): string {
  return `file:${path}`;
}

/** Ask the bar that shows `scope` to open on `query`, from `line`. A query too
 *  short to search asks for nothing, and the file just opens. */
export function requestFind(scope: string, query: string, line: number): void {
  findRequest.value = findQueryChars(query) < MIN_FIND_QUERY_CHARS ? null : { scope, query, line };
}
