import { parseRepoPath } from '../store/store';

/** The path a file-preview overlay is showing, as the user would write it.
 *
 *  A repo-encoded locator (`repo:<repoId>:file:<path>`) is unwrapped to the
 *  repo-relative path it names, so neither the repo id nor the locator's mode
 *  can reach a surface that displays it. A workspace data path passes through
 *  unchanged.
 *
 *  Every surface naming the previewed file resolves it here, so the header
 *  bar's title and the preview's own path row cannot disagree about which file
 *  is open. */
export function previewFilePath(encoded: string): string {
  return parseRepoPath(encoded)?.path ?? encoded;
}

/** Base name of a file-preview path: what the header bar's title renders.
 *
 *  The `|| repoRelative` fallback covers a file at the clone root, which has no
 *  `/` to split on. */
export function previewFileName(encoded: string): string {
  const repoRelative = previewFilePath(encoded);
  return repoRelative.split('/').pop() || repoRelative;
}

/** A preview path split at its last separator, for a surface that renders the
 *  folders and the file name differently.
 *
 *  `dir` KEEPS its trailing slash, so the two halves concatenate back into the
 *  path exactly: a caller rendering them as adjacent spans must not have to
 *  reintroduce a separator that then goes missing for a file at the root, where
 *  `dir` is empty. */
export function splitPreviewPath(encoded: string): { dir: string; name: string } {
  const path = previewFilePath(encoded);
  const cut = path.lastIndexOf('/');
  if (cut < 0) return { dir: '', name: path };
  return { dir: path.slice(0, cut + 1), name: path.slice(cut + 1) };
}
