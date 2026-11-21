/** Pure search logic for FileSearchModal — collects and filters files from all sources. */

export interface FileSearchResult {
  path: string;
  source: 'workspace' | 'repo' | 'change';
  changeStatus?: string;
}

/** The pending changes this surface can actually open.
 *
 *  A `change` row opens through `openRepoFilePreview`, which needs a bound
 *  repository and returns silently without one. Offer them only where they
 *  lead somewhere. */
export function openableChangeFiles<T>(files: T[], hasRepo: boolean): T[] {
  return hasRepo ? files : [];
}

/** Collect searchable files from workspace, repo, and change sources.
 *  Deduplicates within the repo and change categories (workspace paths come
 *  from a disk listing that is already unique); the same path may appear in
 *  different sources (e.g. workspace AND repo) since they're distinct files. */
export function collectSearchResults(
  workspacePaths: string[],
  repoPaths: string[],
  diffFiles: Array<{ path: string; status: string }>,
  ccChangeFiles: Array<{ path: string; status?: string }>,
): FileSearchResult[] {
  const results: FileSearchResult[] = [];
  const seen = new Set<string>();

  for (const p of workspacePaths) {
    results.push({ path: p, source: 'workspace' });
  }

  for (const p of repoPaths) {
    const key = `repo:${p}`;
    if (!seen.has(key)) {
      results.push({ path: p, source: 'repo' });
      seen.add(key);
    }
  }

  for (const f of diffFiles) {
    const key = `change:${f.path}`;
    if (!seen.has(key)) {
      results.push({ path: f.path, source: 'change', changeStatus: f.status });
      seen.add(key);
    }
  }

  for (const f of ccChangeFiles) {
    const key = `change:${f.path}`;
    if (!seen.has(key)) {
      results.push({ path: f.path, source: 'change', changeStatus: f.status });
      seen.add(key);
    }
  }

  return results;
}

/** The most rows the modal renders at once. A workspace holds thousands of
 *  files, and building a row for each one stalls a phone for seconds. */
export const MAX_SHOWN_RESULTS = 100;

/** The matches the modal renders, and how many it left out. Change rows lead,
 *  so a repository's many files never push the pending edits past the cap. */
export function visibleSearchResults(
  results: FileSearchResult[],
): { shown: FileSearchResult[]; hidden: number } {
  const changes = results.filter(r => r.source === 'change');
  const rest = results.filter(r => r.source !== 'change');
  return {
    shown: [...changes, ...rest].slice(0, MAX_SHOWN_RESULTS),
    hidden: Math.max(0, results.length - MAX_SHOWN_RESULTS),
  };
}

/** Filter results by substring match on path (case-insensitive). */
export function filterSearchResults(
  results: FileSearchResult[],
  query: string,
): FileSearchResult[] {
  const needle = query.trim().toLowerCase();
  if (!needle) return results;
  return results.filter(r => r.path.toLowerCase().includes(needle));
}
