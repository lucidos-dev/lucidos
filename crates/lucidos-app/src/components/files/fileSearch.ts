
export interface FileSearchResult {
  path: string;
  source: 'workspace' | 'repo' | 'change';
  changeStatus?: string;
}

export function openableChangeFiles<T>(files: T[], hasRepo: boolean): T[] {
  return hasRepo ? files : [];
}

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

export const MAX_SHOWN_RESULTS = 100;

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

export function filterSearchResults(
  results: FileSearchResult[],
  query: string,
): FileSearchResult[] {
  const needle = query.trim().toLowerCase();
  if (!needle) return results;
  return results.filter(r => r.path.toLowerCase().includes(needle));
}
