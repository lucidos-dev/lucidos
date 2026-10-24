import { computed } from '@preact/signals';
import { previewFile, parseRepoPath, filePreviewSource, filePreviewEditing } from './store';
import { diffBodyKind } from './diffBody';
import { dataPreviewBody, repoPreviewBody } from '../components/files/previewBody';

/** Whether the preview of `path` is the line-numbered source view, the one view
 *  the Wrap toggle acts on.
 *
 *  A picture, a PDF, a rendered document and the inline editor each wrap or
 *  scale on their own. A Wrap control over any of them would be inert. Same
 *  rule `sideBySideDiffAvailable` follows: gate the control on what the body
 *  actually is, never on the overlay alone.
 *
 *  `diffShowsWholeFile` is the caller's, because the two surfaces answer it
 *  differently for a `diff` locator. The Files panel asks `diffBodyKind`, which
 *  can be the hunks. The preview modal always renders the whole file at that
 *  change, so it passes `true` (see `filePreviewModalBody`). */
export function previewShowsSource(path: string, opts: {
  sourceToggle: boolean;
  editing: boolean;
  diffShowsWholeFile: boolean;
}): boolean {
  const repo = parseRepoPath(path);
  if (!repo) {
    return dataPreviewBody(path, {
      sourceToggle: opts.sourceToggle,
      editing: opts.editing,
    }) === 'source';
  }
  if (repo.mode === 'diff' && !opts.diffShowsWholeFile) return false;
  return repoPreviewBody(repo.path, { sourceToggle: opts.sourceToggle }) === 'source';
}

/** Whether the CONTENT PANE's header should offer the Wrap toggle. The modal
 *  asks `previewShowsSource` directly, over its own path: it can be showing a
 *  different file than the pane behind it. */
export const wrapToggleAvailable = computed<boolean>(() => {
  const path = previewFile.value;
  if (!path) return false;
  return previewShowsSource(path, {
    sourceToggle: filePreviewSource.value,
    editing: filePreviewEditing.value,
    diffShowsWholeFile: diffBodyKind.value === 'whole-file',
  });
});
