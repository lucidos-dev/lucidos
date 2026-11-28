
import { findRequest } from '../store';

export const MIN_FIND_QUERY_CHARS = 2;

export function findQueryChars(query: string): number {
  return [...query.trim()].length;
}

export function fileFindScope(path: string): string {
  return `file:${path}`;
}

export function requestFind(scope: string, query: string, line: number): void {
  findRequest.value = findQueryChars(query) < MIN_FIND_QUERY_CHARS ? null : { scope, query, line };
}
