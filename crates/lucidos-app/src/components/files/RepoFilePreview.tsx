import { useEffect, useCallback, useMemo } from 'preact/hooks';
import type { DiffFile, RepoDiff } from '../../store/store';
import { selectedLines, repoDiff, repoPending, filePreviewSource, openImagePopup, repoSelectedChangeId } from '../../store/store';
import type { Loadable } from '../../store/types';
import { getRepoFileContent } from '../../api/client';
import { loadChangeContextById } from '../../store/actions/repositories';
import { highlightFileLines, CODE_EXTS } from '../../utils/syntaxHighlight';
import { escapeHtml } from '../../utils/escapeHtml';
import { renderMarkdown } from '../../utils/renderMarkdown';
import { renderCsvTable } from '../../utils/csv';
import { RENDERABLE_EXTS } from './previewExts';
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

function RepoFileContent({ repoId, path }: { repoId: string; path: string }) {
  const gitRef = repoPending.value?.branch_name;
  const { loadable, showLoading } = useLoadableFetch<string>(
    () => getRepoFileContent(repoId, path, gitRef ?? undefined),
    [repoId, path, gitRef],
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
