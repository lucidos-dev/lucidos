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

export function shouldShowWholeFile(opts: {
  wholeFileOn: boolean;
  fileStatus: DiffFile['status'];
}): boolean {
  if (!opts.wholeFileOn) return false;
  return opts.fileStatus !== 'deleted';
}

export type DiffBodyKind = 'hunks' | 'whole-file' | 'rendered-markdown' | 'no-end-state';

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
    gitRef: repoPending.value?.branch_name ?? null,
    filePreviewSourceOn: filePreviewSource.value,
  });
  return renderedMarkdown ? 'rendered-markdown' : 'hunks';
});

export const sideBySideDiffAvailable = computed<boolean>(
  () => diffBodyKind.value === 'hunks' && diffFitsSideBySide.value,
);
