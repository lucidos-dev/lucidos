import { normalizeDataPath } from './artifacts';
import { parseRepoPath, normalizeLineRange } from '../store';
import { previewMediaKind } from '../../components/files/previewExts';

/** A repo-encoded locator, already parsed. */
export type RepoFileLocator = NonNullable<ReturnType<typeof parseRepoPath>>;

/** What the caller will render for the resolved target.
 *
 *  `as-encoded` renders whatever the locator names, so a `diff` locator renders
 *  the diff. That is the navigate router, and the Files panel behind it.
 *
 *  `file` means the caller renders the FILE whatever the locator names. That is
 *  the app-facing preview modal: the diff view is driven by the Files panel's
 *  global diff state, and loading it would rebind the panel behind the app,
 *  which is the navigation the modal exists to avoid. It is an input rather than
 *  a rewrite of the locator string because the locator's change id is the right
 *  revision to read that file at, and rewriting `diff#<changeId>` into a plain
 *  `file` locator to make the lines honourable is what used to throw it away. */
export type FileTargetView = 'as-encoded' | 'file';

/** A file the UI can show, resolved from an untrusted `file_path` (+ optional
 *  line). Every surface that opens a file from outside the app resolves through
 *  {@link resolveFileTarget}, so they all address exactly the same set of files. */
export interface FileTarget {
  /** The normalized locator: a workspace data path, or a `repo:` encoded path.
   *  Handed to the openers verbatim. */
  path: string;
  /** The parsed repo locator when `path` is repo-encoded, else null. */
  repo: RepoFileLocator | null;
  /** The line range to select and scroll to, or null when the citation names a
   *  line this target cannot honour. */
  range: { start: number; end: number } | null;
}

/** Can a preview of `target` show numbered lines at all?
 *
 *  False for a binary-media file (a PDF, an image, audio, video), which renders
 *  through a URL-pointed element that no `filePreviewSource` toggle turns into
 *  text, and for a repo path in DIFF mode, which renders hunks carrying two sets
 *  of line numbers rather than the file's own. The diff half is scoped to the
 *  `as-encoded` view: a caller that renders the file itself (`view: 'file'`) is
 *  showing the file's own lines, so a citation into it is honourable.
 *
 *  A navigate carrying a line for one of those opens it and stops there. Setting
 *  the selection anyway would leave a highlight nobody can see, and
 *  `currentChatContext` would then attach that invisible range to the user's
 *  next message. A line past the end of a file that IS text can only be caught
 *  once the content loads, so `LineNumberedCode` handles that half. */
function canPreviewLines(path: string, repo: RepoFileLocator | null, view: FileTargetView): boolean {
  if (view === 'as-encoded' && repo?.mode === 'diff') return false;
  const filePath = repo?.path ?? path;
  return previewMediaKind(filePath.split('.').pop()?.toLowerCase() || '') === 'text';
}

/** Resolve a `file` navigation locator into the path to open and the line range
 *  to highlight.
 *
 *  This is the ONE place the locator contract lives, shared by the
 *  `NavigationRequested` router (`handleNavigationRequest`) and the app-facing
 *  file preview modal (`openFilePreviewModal`). Sharing it is load-bearing, not
 *  tidiness: an app iframe is same-origin and could call engine APIs directly,
 *  so the modal must not be able to address a file, or honour a line, that
 *  `navigate('file', …)` would not. A second implementation is how that
 *  guarantee would drift apart.
 *
 *  `line` / `line_end` are `unknown` because they arrive from outside the app
 *  (an app iframe, an LLM `navigate_ui`, an `<a href>` inside a previewed
 *  artifact); `normalizeLineRange` is what rejects anything that isn't a
 *  positive whole number. An unhonourable line never withholds the file: the
 *  range resolves to null and the file opens at the top.
 *
 *  `view` is the ONLY thing the two callers differ by, and it moves exactly one
 *  rule (see `canPreviewLines`). The resolved `path` is identical either way,
 *  which is what makes the reachability guarantee above hold: no view can widen
 *  the set of files a locator addresses. */
export function resolveFileTarget(
  filePath: string,
  line?: unknown,
  lineEnd?: unknown,
  view: FileTargetView = 'as-encoded',
): FileTarget {
  const path = normalizeDataPath(filePath);
  const repo = parseRepoPath(path);
  const range = canPreviewLines(path, repo, view) ? normalizeLineRange(line, lineEnd) : null;
  return { path, repo, range };
}
