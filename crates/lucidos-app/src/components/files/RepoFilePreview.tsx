import { useEffect, useCallback, useMemo } from 'preact/hooks';
import type { DiffFile, RepoDiff } from '../../store/store';
import { selectedLines, repoDiff, repoPending, filePreviewSource, diffWholeFileEffective, openImagePopup, repoSelectedChangeId } from '../../store/store';
import type { Loadable } from '../../store/types';
import { getRepoFileContent, getChangeFileContent, repoFileUrl, changeFileUrl } from '../../api/client';
import { loadChangeContextById } from '../../store/actions/repositories';
import { highlightFileLines, CODE_EXTS } from '../../utils/syntaxHighlight';
import { escapeHtml } from '../../utils/escapeHtml';
import { renderMarkdown } from '../../utils/renderMarkdown';
import { renderCsvTable } from '../../utils/csv';
import { RENDERABLE_EXTS, previewMediaKind } from './previewExts';
import { isMobile, viewportIsMobile } from '../../utils/viewport';
import { useLoadableFetch } from '../../hooks/useLoadableFetch';
import { useDelayedLoading } from '../../hooks/useDelayedLoading';
import { DiffView } from './DiffView';
import { RenderedDiff } from './RenderedDiff';
import { ChangesFileList } from './RepoFilesView';
import { LoadableError } from '../shared/LoadableError';

interface Props {
  repoId: string;
  mode: 'file' | 'diff';
  path: string;
  /** When the overlay was restored from nav history after a reload, repoDiff
   *  is empty — this is the change to fetch to repopulate it. */
  changeId?: string;
  /** Skip mounting in the inactive dual-rendered layout — otherwise both
   *  SplitLayout and MobileSwipeContainer copies fetch and decode the file. */
  layout: 'desktop' | 'mobile';
}

/** Decide whether to render a markdown diff via RenderedDiff (vs raw DiffView).
 *  RenderedDiff needs to fetch the post-change file body, which requires either
 *  a pending `Change` row (changeId → /api/v1/changes/:id/file — covers
 *  Lucidos-internal AND app coding-agent threads, both of which produce a
 *  Change) or a CC worktree branch on a registered repo (branchRef →
 *  /api/v1/repositories/:id/file?ref=). External-repo Claude Code sessions
 *  skip the Apply flow and only ever have the branchRef path. */
export function shouldRenderMarkdownDiff(opts: {
  ext: string;
  fileStatus: DiffFile['status'];
  activeChangeId: string | null;
  branchRef: string | null;
  filePreviewSourceOn: boolean;
}): boolean {
  if (opts.filePreviewSourceOn) return false;
  if (opts.ext !== 'md') return false;
  if (opts.fileStatus === 'deleted') return false;
  return !!opts.activeChangeId || !!opts.branchRef;
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

/** `hidden` covers both not-loaded and loaded-with-zero-files — the inner
 *  pane renders alone. Loading and failed keep the sidebar mounted so the
 *  in-flight fetch / server error is visible to the user. */
export type SidebarState =
  | { kind: 'hidden' }
  | { kind: 'loading' }
  | { kind: 'failed'; error: string }
  | { kind: 'files'; files: DiffFile[] };

export function sidebarStateFromDiff(diff: Loadable<RepoDiff>): SidebarState {
  if (diff.status === 'loading') return { kind: 'loading' };
  if (diff.status === 'failed') return { kind: 'failed', error: diff.error };
  if (diff.status === 'not-loaded') return { kind: 'hidden' };
  if (diff.data.files.length === 0) return { kind: 'hidden' };
  return { kind: 'files', files: diff.data.files };
}

/** Renders RepoFilePreview with a sidebar listing the changed files in the
 *  current diff. The sidebar is hidden via container query when the content
 *  pane is too narrow (see `.repo-preview-split-sidebar` in panels.css), so
 *  mobile and a heavily-collapsed content pane fall back to today's behavior. */
export function RepoFilePreviewWithSidebar(props: Props) {
  const isActiveLayout = props.layout === (viewportIsMobile.value ? 'mobile' : 'desktop');
  if (!isActiveLayout) return null;

  const sidebar = sidebarStateFromDiff(repoDiff.value);

  if (sidebar.kind === 'hidden') {
    return <RepoFilePreview {...props} />;
  }

  return (
    <div class="repo-preview-split">
      <aside class="repo-preview-split-sidebar">
        {sidebar.kind === 'loading' && (
          <div class="repo-preview-sidebar-state loading-skeleton" data-state="loading">
            Loading changed files…
          </div>
        )}
        {sidebar.kind === 'failed' && (
          <div class="repo-preview-sidebar-state repo-preview-sidebar-error" data-state="failed">
            Failed to load: {sidebar.error}
          </div>
        )}
        {sidebar.kind === 'files' && (
          <ChangesFileList files={sidebar.files} activePath={props.path} />
        )}
      </aside>
      <div class="repo-preview-split-main">
        <RepoFilePreview {...props} />
      </div>
    </div>
  );
}

function RepoFilePreview({ repoId, mode, path, changeId, layout }: Props) {
  const isActiveLayout = layout === (viewportIsMobile.value ? 'mobile' : 'desktop');
  const showDiffLoading = useDelayedLoading(repoDiff.value);

  // After a reload, the panel overlay re-hydrates from nav history but the
  // repoDiff/repoSelectedChangeId backing state does not. If the URL carries
  // the change ID and the runtime state is stale, refetch the change context.
  // Gated on isActiveLayout so the inactive dual-rendered copy doesn't fire
  // a duplicate fetch in the same tick.
  useEffect(() => {
    if (!isActiveLayout) return;
    if (mode !== 'diff' || !changeId) return;
    if (repoSelectedChangeId.value === changeId && repoDiff.value.status === 'loaded') return;
    void loadChangeContextById(changeId);
  }, [mode, changeId, isActiveLayout]);

  if (!isActiveLayout) return null;

  if (mode === 'diff') {
    const diff = repoDiff.value;
    if (diff.status === 'failed') return <LoadableError noun="diff" error={diff.error} />;
    if (diff.status !== 'loaded') return showDiffLoading ? <div class="loading-spinner" /> : null;
    const file = diff.data.files.find(f => f.path === path);
    if (!file) return <div class="empty-state">File not found in diff</div>;

    const ext = path.split('.').pop()?.toLowerCase() || '';
    const activeChangeId = changeId ?? repoSelectedChangeId.value;
    const branchRef = repoPending.value?.branch_name ?? null;

    // Whole-file mode: show the file as it would be once merged (end state),
    // rendered like the All Files view. A deletion has no end state. Added files
    // default to this view (their diff is all additions); see diffWholeFileEffective.
    const wholeFileOn = diffWholeFileEffective.value;
    if (wholeFileOn) {
      if (shouldShowWholeFile({ wholeFileOn, fileStatus: file.status })) {
        return <RepoFileContent repoId={repoId} path={path} changeId={activeChangeId ?? undefined} />;
      }
      return <div class="empty-state">File is deleted in this change — no end state to show</div>;
    }

    if (shouldRenderMarkdownDiff({
      ext,
      fileStatus: file.status,
      activeChangeId,
      branchRef,
      filePreviewSourceOn: filePreviewSource.value,
    })) {
      return <RenderedDiff file={file} changeId={activeChangeId} repoId={repoId} gitRef={branchRef} />;
    }
    return <DiffView file={file} />;
  }

  return <RepoFileContent repoId={repoId} path={path} />;
}

/** Dispatches a repo file to the right preview. Binary-media files (images,
 *  video, audio, pdf) are pointed at the file URL via a media element — the
 *  engine serves them with a content-type from the extension. Everything else
 *  (source, markdown, csv, svg, and any extensionless/unknown-but-textual file)
 *  goes through the text path, which fetches the body as a string. Fetching a
 *  PNG as text was the bug: it rendered the raw bytes line-numbered. */
function RepoFileContent({ repoId, path, changeId }: { repoId: string; path: string; changeId?: string }) {
  const ext = path.split('.').pop()?.toLowerCase() || '';
  if (previewMediaKind(ext) !== 'text') {
    return <RepoFileMedia repoId={repoId} path={path} changeId={changeId} ext={ext} />;
  }
  return <RepoFileText repoId={repoId} path={path} changeId={changeId} />;
}

/** Binary-media preview. Builds the file URL (same change-vs-branch ref logic as
 *  RepoFileText) and renders it without fetching the bytes as text. */
function RepoFileMedia({ repoId, path, changeId, ext }: { repoId: string; path: string; changeId?: string; ext: string }) {
  const gitRef = repoPending.value?.branch_name;
  const url = changeId ? changeFileUrl(changeId, path) : repoFileUrl(repoId, path, gitRef ?? undefined);
  const kind = previewMediaKind(ext);

  if (kind === 'image') {
    return <img src={url} alt={path} style="max-width:100%;max-height:100%;object-fit:contain;" onClick={() => { if (isMobile()) openImagePopup(url); }} />;
  }
  if (kind === 'pdf') return <iframe src={url} style="width:100%;height:100%;border:none;" />;
  if (kind === 'video') return <video src={url} controls style="max-width:100%;max-height:100%;" />;
  return <audio src={url} controls style="width:100%;" />;
}

function RepoFileText({ repoId, path, changeId }: { repoId: string; path: string; changeId?: string }) {
  const gitRef = repoPending.value?.branch_name;
  // With a Lucidos/app change row, fetch the end state via /changes/:id/file —
  // the correct ref for both pending (branch) and applied (post_merge_sha). Without
  // one (external-repo CC), fall back to the branch ref. Mirrors RenderedDiff.
  const { loadable, showLoading } = useLoadableFetch<string>(
    () => changeId
      ? getChangeFileContent(changeId, path)
      : getRepoFileContent(repoId, path, gitRef ?? undefined),
    [repoId, path, changeId, gitRef],
  );

  const handleLineClick = useCallback((lineNum: number, shiftKey: boolean) => {
    if (shiftKey && selectedLines.value) {
      selectedLines.value = {
        start: Math.min(selectedLines.value.start, lineNum),
        end: Math.max(selectedLines.value.end, lineNum),
      };
    } else {
      selectedLines.value = { start: lineNum, end: lineNum };
    }
  }, []);

  const ext = path.split('.').pop()?.toLowerCase() || '';
  const content = loadable.status === 'loaded' ? loadable.data : null;
  const renderPreview = content !== null && !filePreviewSource.value && RENDERABLE_EXTS.includes(ext);
  const isCode = CODE_EXTS.includes(ext);

  const renderedHtml = useMemo(() => {
    if (!content || !renderPreview) return null;
    if (ext === 'md') return renderMarkdown(content);
    if (ext === 'csv') return renderCsvTable(content);
    if (ext === 'svg') return URL.createObjectURL(new Blob([content], { type: 'image/svg+xml' }));
    return null;
  }, [content, ext, renderPreview]);

  useEffect(() => {
    if (renderedHtml && ext === 'svg') return () => URL.revokeObjectURL(renderedHtml);
  }, [renderedHtml, ext]);

  const highlightedLines = useMemo(
    () => content ? (isCode ? highlightFileLines(content, ext) : content.split('\n').map(escapeHtml)) : [],
    [content, ext, isCode],
  );

  if (loadable.status === 'failed') return <LoadableError noun="file" error={loadable.error} />;
  if (content === null) return showLoading ? <div class="loading-spinner" /> : null;

  if (renderPreview) {
    if (ext === 'md') return <div class="response-content markdown-content" dangerouslySetInnerHTML={{ __html: renderedHtml! }} />;
    if (ext === 'html' || ext === 'htm') return <iframe srcDoc={content} style="width:100%;height:100%;border:none;background:#fff;" />;
    if (ext === 'csv') return <div dangerouslySetInnerHTML={{ __html: renderedHtml! }} />;
    if (ext === 'svg') return <img src={renderedHtml!} alt={path} style="max-width:100%;max-height:100%;object-fit:contain;" onClick={() => { if (isMobile()) openImagePopup(renderedHtml!); }} />;
  }

  const sel = selectedLines.value;

  return (
    <div class="repo-file-content">
      <pre class="file-preview-code line-numbered">
        {highlightedLines.map((html, i) => {
          const num = i + 1;
          const isSelected = sel !== null && num >= sel.start && num <= sel.end;

          return (
            <div key={num} class={`code-line ${isSelected ? 'line-selected' : ''}`}>
              <span
                class="line-number"
                onClick={(e: MouseEvent) => handleLineClick(num, e.shiftKey)}
              >
                {num}
              </span>
              <span class="line-content" dangerouslySetInnerHTML={{ __html: html || ' ' }} />
            </div>
          );
        })}
      </pre>
    </div>
  );
}
