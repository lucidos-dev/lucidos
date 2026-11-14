import type { DiffFile } from '../../store/store';

export function diffStats(file: DiffFile): { additions: number; deletions: number } {
  let additions = 0;
  let deletions = 0;
  for (const hunk of file.hunks) {
    for (const line of hunk.lines) {
      if (line.type === 'addition') additions++;
      else if (line.type === 'deletion') deletions++;
    }
  }
  return { additions, deletions };
}
