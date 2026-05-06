import { useState, useEffect, useCallback, useMemo } from 'preact/hooks';
import { selectedLines, repoDiff, repoPending, filePreviewSource, popupImageSrc, repoSelectedChangeId } from '../../store/store';
import { getRepoFileContent } from '../../api/client';
import { loadChangeContextById } from '../../store/actions/repositories';
import { highlightFileLines, CODE_EXTS } from '../../utils/syntaxHighlight';
import { escapeHtml } from '../../utils/escapeHtml';
import { renderMarkdown } from '../../utils/renderMarkdown';
import { renderCsvTable } from '../../utils/csv';
import { RENDERABLE_EXTS } from './FilePreviewInline';
import { isMobile } from '../../utils/viewport';
import { DiffView } from './DiffView';
import { RenderedDiff } from './RenderedDiff';

interface Props {
  repoId: string;
  mode: 'file' | 'diff';
  path: string;
  /** When the overlay was restored from nav history after a reload, repoDiff
   *  is empty — this is the change to fetch to repopulate it. */
  changeId?: string;
}

export function RepoFilePreview({ repoId, mode, path, changeId }: Props) {
  // After a reload, the panel overlay re-hydrates from nav history but the
  // repoDiff/repoSelectedChangeId backing state does not. If the URL carries
  // the change ID and the runtime state is stale, refetch the change context.
  useEffect(() => {
    if (mode !== 'diff' || !changeId) return;
    if (repoSelectedChangeId.value === changeId && repoDiff.value.status === 'loaded') return;
    loadChangeContextById(changeId);
  }, [mode, changeId]);

  if (mode === 'diff') {
    const diff = repoDiff.value;
    if (diff.status === 'failed') return <div class="empty-state" style="color:var(--accent-red)">Failed to load diff: {diff.error}</div>;
    if (diff.status !== 'loaded') return <div class="loading-spinner" />;
    const file = diff.data.files.find(f => f.path === path);
    if (!file) return <div class="empty-state">File not found in diff</div>;

    const ext = path.split('.').pop()?.toLowerCase() || '';
    const activeChangeId = changeId ?? repoSelectedChangeId.value;
    const wantRendered = !filePreviewSource.value && ext === 'md' && activeChangeId && file.status !== 'deleted';
    if (wantRendered) return <RenderedDiff file={file} changeId={activeChangeId} />;
    return <DiffView file={file} />;
  }

  return <RepoFileContent repoId={repoId} path={path} />;
}

function RepoFileContent({ repoId, path }: { repoId: string; path: string }) {
  const [content, setContent] = useState<string | null>(null);
  const [error, setError] = useState<string | null>(null);
  const gitRef = repoPending.value?.branch_name;

  useEffect(() => {
    setContent(null);
    setError(null);
    getRepoFileContent(repoId, path, gitRef ?? undefined)
      .then(setContent)
      .catch(e => setError(e.message));
  }, [repoId, path, gitRef]);

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

  if (error) return <div class="empty-state" style="color:var(--accent-red)">Failed to load: {error}</div>;
  if (content === null) return <div class="loading-spinner" />;

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
