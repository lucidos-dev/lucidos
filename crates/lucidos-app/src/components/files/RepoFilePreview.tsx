import { useEffect, useCallback, useMemo } from 'preact/hooks';
import type { DiffFile } from '../../store/store';
import { selectedLines, repoDiff, repoPending, filePreviewSource, popupImageSrc, repoSelectedChangeId } from '../../store/store';
import { getRepoFileContent } from '../../api/client';
import { loadChangeContextById } from '../../store/actions/repositories';
import { highlightFileLines, CODE_EXTS } from '../../utils/syntaxHighlight';
import { escapeHtml } from '../../utils/escapeHtml';
import { renderMarkdown } from '../../utils/renderMarkdown';
import { renderCsvTable } from '../../utils/csv';
import { RENDERABLE_EXTS } from './FilePreviewInline';
import { isMobile, viewportIsMobile } from '../../utils/viewport';
import { useLoadableFetch } from '../../hooks/useLoadableFetch';
import { DiffView } from './DiffView';
import { RenderedDiff } from './RenderedDiff';
import { ChangesFileList } from './RepoFilesView';

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
 *  a Lucidos `Change` row (changeId → /api/changes/:id/file) or a CC worktree
 *  branch on a registered repo (branchRef → /api/repositories/:id/file?ref=).
 *  External-repo CC sessions only ever have the branchRef path. */
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

/** Renders RepoFilePreview with a sidebar listing the changed files in the
 *  current diff. The sidebar is hidden via container query when the content
 *  pane is too narrow (see `.repo-preview-split-sidebar` in panels.css), so
 *  mobile and a heavily-collapsed content pane fall back to today's behavior. */
export function RepoFilePreviewWithSidebar(props: Props) {
  const isActiveLayout = props.layout === (viewportIsMobile.value ? 'mobile' : 'desktop');
  if (!isActiveLayout) return null;

  const diff = repoDiff.value;
  const files = diff.status === 'loaded' ? diff.data.files : [];
  const showSidebar = files.length > 0;

  if (!showSidebar) {
    return <RepoFilePreview {...props} />;
  }

  return (
    <div class="repo-preview-split">
      <aside class="repo-preview-split-sidebar">
        <ChangesFileList files={files} activePath={props.path} />
      </aside>
      <div class="repo-preview-split-main">
        <RepoFilePreview {...props} />
      </div>
    </div>
  );
}

export function RepoFilePreview({ repoId, mode, path, changeId, layout }: Props) {
  const isActiveLayout = layout === (viewportIsMobile.value ? 'mobile' : 'desktop');

  // After a reload, the panel overlay re-hydrates from nav history but the
  // repoDiff/repoSelectedChangeId backing state does not. If the URL carries
  // the change ID and the runtime state is stale, refetch the change context.
  // Gated on isActiveLayout so the inactive dual-rendered copy doesn't fire
  // a duplicate fetch in the same tick.
  useEffect(() => {
    if (!isActiveLayout) return;
    if (mode !== 'diff' || !changeId) return;
    if (repoSelectedChangeId.value === changeId && repoDiff.value.status === 'loaded') return;
    loadChangeContextById(changeId);
  }, [mode, changeId, isActiveLayout]);

  if (!isActiveLayout) return null;

  if (mode === 'diff') {
    const diff = repoDiff.value;
    if (diff.status === 'failed') return <div class="empty-state error-text">Failed to load diff: {diff.error}</div>;
    if (diff.status !== 'loaded') return <div class="loading-spinner" />;
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

  if (loadable.status === 'failed') return <div class="empty-state error-text">Failed to load: {loadable.error}</div>;
  if (content === null) return showLoading ? <div class="loading-spinner" /> : null;

  if (renderPreview) {
    if (ext === 'md') return <div class="response-content markdown-content" dangerouslySetInnerHTML={{ __html: renderedHtml! }} />;
    if (ext === 'html' || ext === 'htm') return <iframe srcDoc={content} style="width:100%;height:100%;border:none;background:#fff;" />;
    if (ext === 'csv') return <div dangerouslySetInnerHTML={{ __html: renderedHtml! }} />;
    if (ext === 'svg') return <img src={renderedHtml!} alt={path} style="max-width:100%;max-height:100%;object-fit:contain;" onClick={() => { if (isMobile()) popupImageSrc.value = renderedHtml!; }} />;
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
