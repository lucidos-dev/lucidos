import { computed } from '@preact/signals';
import { previewFile, parseRepoPath, filePreviewSource, filePreviewEditing } from './store';
import { diffBodyKind } from './diffBody';
import { dataPreviewBody, repoPreviewBody } from '../components/files/previewBody';

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

export const wrapToggleAvailable = computed<boolean>(() => {
  const path = previewFile.value;
  if (!path) return false;
  return previewShowsSource(path, {
    sourceToggle: filePreviewSource.value,
    editing: filePreviewEditing.value,
    diffShowsWholeFile: diffBodyKind.value === 'whole-file',
  });
});
