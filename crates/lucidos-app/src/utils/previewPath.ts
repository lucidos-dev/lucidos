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

/** Where the previewed file actually lives on this machine, or null when that
 *  cannot be answered yet.
 *
 *  Only the packaged desktop client can use this: no page may navigate to
 *  `file://`, so a browser has to go through the engine's `/data/` mount
 *  instead. There the OS opener takes over, and an `.html` lands in the default
 *  browser while a `.png` lands in the image viewer.
 *
 *  Two shapes, because the two locators name two different trees. A repo file is
 *  relative to its clone; a workspace data path is relative to `data/` under the
 *  workspace. The caller already holds both roots: `workspacePath` from
 *  `/health`, and `Repository.path` from the repositories list. Either can still
 *  be missing while its own load is in flight.
 *
 *  `system-knowhow/` is the one data prefix with no file under the workspace at
 *  all: it ships inside the engine and is served from there, so it answers null
 *  and the caller falls back to the URL.
 *
 *  Pure, and takes its roots as arguments rather than reading the signals, so
 *  the two joins are testable without a store. */
export function previewDiskPath(
  encoded: string,
  workspacePath: string,
  repositories: readonly { id: string; path: string }[],
): string | null {
  const repo = parseRepoPath(encoded);
  if (repo) {
    const clone = repositories.find(r => r.id === repo.repoId);
    return clone?.path ? joinPath(clone.path, repo.path) : null;
  }
  if (!workspacePath || encoded.startsWith('system-knowhow/')) return null;
  return joinPath(workspacePath, `data/${encoded}`);
}

/** What "copy the path" puts on the clipboard: the absolute disk path where
 *  {@link previewDiskPath} can resolve one, else the repo- or
 *  workspace-relative path the locator itself names. `absolute` says which one
 *  it got, so the caller's confirmation can say so too.
 *
 *  Pure, for the same reason `previewDiskPath` is: the roots are arguments, not
 *  signals read here. */
export function previewCopyPath(
  encoded: string,
  workspacePath: string,
  repositories: readonly { id: string; path: string }[],
): { path: string; absolute: boolean } {
  const disk = previewDiskPath(encoded, workspacePath, repositories);
  return disk ? { path: disk, absolute: true } : { path: previewFilePath(encoded), absolute: false };
}

/** Join an absolute root to a relative path, tolerating a trailing slash on the
 *  root: `workspace_path` comes off the wire and a caller may register a clone
 *  either way. */
function joinPath(root: string, relative: string): string {
  return `${root.replace(/\/+$/, '')}/${relative}`;
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
