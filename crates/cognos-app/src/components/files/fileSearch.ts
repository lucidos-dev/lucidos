/** Pure search logic for FileSearchModal — collects and filters files from all sources. */

export interface FileSearchResult {
  path: string;
  source: 'workspace' | 'repo' | 'change';
  changeStatus?: string;
}

/** Collect searchable files from workspace, repo, and change sources.
 *  Deduplicates within each source category; same path may appear in
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
    seen.add(p);
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

/** Filter results by substring match on path (case-insensitive). */
export function filterSearchResults(
  results: FileSearchResult[],
  query: string,
): FileSearchResult[] {
  const needle = query.trim().toLowerCase();
  if (!needle) return results;
  return results.filter(r => r.path.toLowerCase().includes(needle));
}
