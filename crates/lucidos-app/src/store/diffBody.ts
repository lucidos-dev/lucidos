import { computed } from '@preact/signals';
import {
  previewFile,
  parseRepoPath,
  repoDiff,
  repoPending,
  repoSelectedChangeId,
  filePreviewSource,
  diffWholeFileEffective,
  diffFitsSideBySide,
  type DiffFile,
} from './store';

/** Decide whether to render a markdown diff via RenderedDiff (vs raw DiffView).
 *  RenderedDiff needs to fetch the post-change file body, which requires either
 *  a pending `Change` row (changeId, /api/v1/changes/:id/file, which covers
 *  Lucidos-internal AND app coding-agent threads, both of which produce a
 *  Change) or a git ref on a registered repo (gitRef,
 *  /api/v1/repositories/:id/file?ref=). External-repo Claude Code sessions
 *  skip the Apply flow and only ever have the gitRef path. */
export function shouldRenderMarkdownDiff(opts: {
  ext: string;
  fileStatus: DiffFile['status'];
  activeChangeId: string | null;
  gitRef: string | null;
  filePreviewSourceOn: boolean;
}): boolean {
  if (opts.filePreviewSourceOn) return false;
  if (opts.ext !== 'md') return false;
  if (opts.fileStatus === 'deleted') return false;
  return !!opts.activeChangeId || !!opts.gitRef;
}

/** Whether the diff preview should show the whole merged end-state file instead
 *  of the unified hunks. A deletion has no end state, so the whole-file view is
 *  suppressed for deleted files even when the toggle is on. */
export function shouldShowWholeFile(opts: {
  wholeFileOn: boolean;
  fileStatus: DiffFile['status'];
}): boolean {
  if (!opts.wholeFileOn) return false;
  return opts.fileStatus !== 'deleted';
}

/** Which body the diff preview is showing.
 *
 *    hunks             the diff itself, unified or (with room, and the toggle
 *                      on) side by side
 *    whole-file        the merged end-state file, via the "Show full file" toggle
 *    rendered-markdown the post-change markdown with change marks
 *    no-end-state      a deleted file with the whole-file toggle on: nothing to show
 */
export type DiffBodyKind = 'hunks' | 'whole-file' | 'rendered-markdown' | 'no-end-state';

/** The body the diff preview is showing, or null when it is not showing a diff
 *  at all (no preview, a file locator, or the diff not loaded yet).
 *
 *  Derived once here because the header and the body must AGREE on it: the
 *  header offers "Side by side" only for the raw hunks, and offering it over a
 *  rendered markdown diff or the whole-file view would be a control that does
 *  nothing. `diffWholeFileEffective` exists for the same reason and is one of
 *  the inputs; this is the same move one level up, composing the two pure
 *  predicates above rather than restating either. */
export const diffBodyKind = computed<DiffBodyKind | null>(() => {
  const encoded = previewFile.value;
  if (!encoded) return null;
  const parsed = parseRepoPath(encoded);
  if (!parsed || parsed.mode !== 'diff') return null;
  const diff = repoDiff.value;
  if (diff.status !== 'loaded') return null;
  const file = diff.data.files.find(f => f.path === parsed.path);
  if (!file) return null;

  const wholeFileOn = diffWholeFileEffective.value;
  if (wholeFileOn) {
    return shouldShowWholeFile({ wholeFileOn, fileStatus: file.status }) ? 'whole-file' : 'no-end-state';
  }

  const renderedMarkdown = shouldRenderMarkdownDiff({
    ext: parsed.path.split('.').pop()?.toLowerCase() || '',
    fileStatus: file.status,
    activeChangeId: parsed.changeId ?? repoSelectedChangeId.value,
    // A diff locator carries a change id rather than a git ref, so the ref is
    // always the bound repository's pending coding-agent branch here. Same
    // value `RepoFilePreview` resolves through `previewGitRef`.
    gitRef: repoPending.value?.branch_name ?? null,
    filePreviewSourceOn: filePreviewSource.value,
  });
  return renderedMarkdown ? 'rendered-markdown' : 'hunks';
});

/** Whether the header should offer the "Side by side" toggle.
 *
 *  Side by side is a rendering of the HUNKS, so it is meaningless over the whole
 *  merged file or the rendered markdown diff, and two columns of unwrapped code
 *  need width the content pane may not have (`diffFitsSideBySide`, measured by
 *  `DiffView`). A control that is present but inert is a lie about what the
 *  surface can do, so both conditions gate the control, not just the rendering. */
export const sideBySideDiffAvailable = computed<boolean>(
  () => diffBodyKind.value === 'hunks' && diffFitsSideBySide.value,
);
