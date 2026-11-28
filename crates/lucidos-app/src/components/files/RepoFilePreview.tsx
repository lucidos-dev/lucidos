import { useEffect, useMemo, useState } from 'preact/hooks';
import type { DiffFile, RepoDiff, RepoLocator } from '../../store/store';
import { repoDiff, repoPending, filePreviewWrap, diffSideBySide, repoSelectedChangeId, openFilePreviewRevision, encodeRepoPath } from '../../store/store';
import { diffBodyKind } from '../../store/diffBody';
import type { Loadable } from '../../store/types';
import { getRepoFileContent, getChangeFileContent, repoFileUrl, changeFileUrl } from '../../api/client';
import { loadChangeContextById } from '../../store/actions/repositories';
import { refreshFilePreview, registerPreviewTextBody, reportPreviewTextSettled } from '../../store/actions/artifacts';
import { usePanelRefresh } from '../../hooks/usePanelRefresh';
import { syntaxHighlighter, highlightFileLines, CODE_EXTS } from '../../utils/syntaxHighlight';
import { escapeHtml } from '../../utils/escapeHtml';
import { MarkdownDocument } from './MarkdownDocument';
import { handlePreviewLinkClick } from './previewIframeLinks';
import { renderCsvTable } from '../../utils/csv';
import { PreviewImage } from './PreviewImage';
import { previewExt, repoPreviewBody } from './previewBody';
import { viewportIsMobile } from '../../utils/viewport';
import { useLoadableFetch } from '../../hooks/useLoadableFetch';
import { useDelayedLoading } from '../../hooks/useDelayedLoading';
import { DiffSkeleton, DiffView } from './DiffView';
import { RenderedDiff } from './RenderedDiff';
import { ChangesFileList, ChangesFileListSkeleton } from './RepoFilesView';
import { RepoPreviewSplit } from './RepoPreviewSplit';
import { LoadableError } from '../shared/LoadableError';
import { LineNumberedCode, fileRows } from './LineNumberedCode';
import { FileSourceSkeleton, ProseSkeleton } from './previewSkeletons';
import { LoadingFade } from '../shared/LoadingFade';
import { bridgePreviewIframeShortcuts } from './previewIframeShortcuts';
import { withPreviewRevision } from './previewRevision';
import { usePreviewViewState } from './previewViewState';
import { IframeTabExit } from '../shared/IframeTabExit';

interface Props {
  locator: RepoLocator;
  layout: 'desktop' | 'mobile';
}

export function previewGitRef(locator: RepoLocator, surfaceDefault: string | null): string | null {
  return (locator.mode === 'file' ? locator.ref : undefined) ?? surfaceDefault;
}

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

export function RepoFilePreviewWithSidebar(props: Props) {
  const isActiveLayout = props.layout === (viewportIsMobile.value ? 'mobile' : 'desktop');
  const showSidebarLoading = useDelayedLoading(repoDiff.value);
  const refreshable = isActiveLayout && props.locator.mode !== 'diff';
  usePanelRefresh(`file "${props.locator.path}"`, refreshable ? refreshFilePreview : null);
  if (!isActiveLayout) return null;

  const sidebar = sidebarStateFromDiff(repoDiff.value);

  if (sidebar.kind === 'hidden') {
    return <RepoFilePreview {...props} />;
  }

  return (
    <RepoPreviewSplit
      sidebar={sidebar.kind === 'failed' ? (
        <div class="repo-preview-sidebar-state repo-preview-sidebar-error" data-state="failed">
          Failed to load: {sidebar.error}
        </div>
      ) : (
        <LoadingFade showSkeleton={sidebar.kind === 'loading' && showSidebarLoading} skeleton={<ChangesFileListSkeleton />}>
          {sidebar.kind === 'files' && (
            <ChangesFileList files={sidebar.files} activePath={props.locator.path} />
          )}
        </LoadingFade>
      )}
      main={<RepoFilePreview {...props} />}
    />
  );
}

function RepoFilePreview({ locator, layout }: Props) {
  const { repoId, mode, path } = locator;
  const changeId = locator.mode === 'diff' ? locator.changeId : undefined;
  const isActiveLayout = layout === (viewportIsMobile.value ? 'mobile' : 'desktop');
  const showDiffLoading = useDelayedLoading(repoDiff.value);
  const gitRef = previewGitRef(locator, repoPending.value?.branch_name ?? null);
  const revision = openFilePreviewRevision.value;

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
    return (
      <LoadingFade class="repo-preview-fade" showSkeleton={showDiffLoading} skeleton={<DiffSkeleton />}>
        {diff.status === 'loaded' && diffBody(diff.data.files)}
      </LoadingFade>
    );
  }

  return <RepoFileContent repoId={repoId} path={path} gitRef={gitRef} revision={revision} />;

  function diffBody(files: DiffFile[]) {
    const file = files.find(f => f.path === path);
    if (!file) return <div class="empty-state">File not found in diff</div>;

    const activeChangeId = changeId ?? repoSelectedChangeId.value;

    switch (diffBodyKind.value) {
      case 'whole-file':
        return <RepoFileContent repoId={repoId} path={path} changeId={activeChangeId ?? undefined} gitRef={gitRef} revision={revision} />;
      case 'no-end-state':
        return <div class="empty-state">File is deleted in this change, so there is no end state to show</div>;
      case 'rendered-markdown':
        return <RenderedDiff file={file} changeId={activeChangeId} repoId={repoId} gitRef={gitRef} />;
      default:
        return <DiffView file={file} sideBySide={diffSideBySide.value} measureFit />;
    }
  }
}

interface RepoFileContentProps {
  repoId: string;
  path: string;
  changeId?: string;
  gitRef: string | null;
  revision?: number;
}

export function RepoFileContent(props: RepoFileContentProps) {
  return repoFileContentLeaf(props, usePreviewViewState().source.value);
}

export function repoFileContentLeaf({ repoId, path, changeId, gitRef, revision }: RepoFileContentProps, sourceToggle: boolean) {
  const body = repoPreviewBody(path, { sourceToggle });
  if (body === 'image' || body === 'pdf' || body === 'video' || body === 'audio') {
    return <RepoFileMedia repoId={repoId} path={path} changeId={changeId} gitRef={gitRef} revision={revision} kind={body} />;
  }
  return <RepoFileText key={`${repoId}:${changeId ?? ''}:${gitRef ?? ''}:${path}`} repoId={repoId} path={path} changeId={changeId} gitRef={gitRef} revision={revision} body={body} />;
}

function RepoFileMedia({ repoId, path, changeId, gitRef, revision, kind }: RepoFileContentProps & { kind: 'image' | 'pdf' | 'video' | 'audio' }) {
  const url = withPreviewRevision(
    changeId ? changeFileUrl(changeId, path) : repoFileUrl(repoId, path, gitRef ?? undefined),
    revision ?? 0,
  );

  if (kind === 'image') return <RepoImage key={url} src={url} path={path} />;
  if (kind === 'pdf') return <><iframe src={url} style="width:100%;height:100%;border:none;" onLoad={(e) => bridgePreviewIframeShortcuts(e.currentTarget)} /><IframeTabExit /></>;
  if (kind === 'video') return <video src={url} controls style="max-width:100%;max-height:100%;" />;
  return <audio src={url} controls style="width:100%;" />;
}

function RepoImage({ src, path }: { src: string; path: string }) {
  const [failed, setFailed] = useState(false);
  if (failed) return <LoadableError noun="image" error={`${path} could not be read from the repository`} />;
  return <PreviewImage src={src} alt={path} onError={() => setFailed(true)} />;
}

function RepoFileText({ repoId, path, changeId, gitRef, revision, body }: RepoFileContentProps & { body: 'markdown' | 'csv' | 'source' }) {
  const servesPanel = revision !== undefined;
  useEffect(() => (servesPanel ? registerPreviewTextBody() : undefined), [servesPanel]);
  const { loadable, showLoading } = useLoadableFetch<string>(
    () => changeId
      ? getChangeFileContent(changeId, path)
      : getRepoFileContent(repoId, path, gitRef ?? undefined),
    [repoId, path, changeId, gitRef, revision],
    { keepLoadedWhileRefetching: servesPanel, onSettled: servesPanel ? () => reportPreviewTextSettled(revision ?? 0) : undefined },
  );

  const ext = previewExt(path);
  const content = loadable.status === 'loaded' ? loadable.data : null;
  const isCode = CODE_EXTS.includes(ext);

  const csvHtml = useMemo(
    () => (content && body === 'csv' ? renderCsvTable(content) : null),
    [content, body],
  );

  const rows = useMemo(
    () => fileRows(content ? (isCode ? highlightFileLines(content, ext) : content.split('\n').map(escapeHtml)) : []),
    [content, ext, isCode, syntaxHighlighter.value],
  );

  if (loadable.status === 'failed') return <LoadableError noun="file" error={loadable.error} />;
  return (
    <LoadingFade
      class="repo-preview-fade"
      showSkeleton={showLoading}
      skeleton={body === 'source'
        ? <div class="repo-file-content"><FileSourceSkeleton wideLines={filePreviewWrap.value ? 'wrap' : 'pan'} /></div>
        : <div class="repo-file-rendered"><ProseSkeleton /></div>}
    >
      {content !== null && loadedBody()}
    </LoadingFade>
  );

  function loadedBody() {
    if (body === 'markdown') {
      const locator: RepoLocator = { repoId, mode: 'file', ref: gitRef ?? undefined, path };
      return (
        <div class="repo-file-rendered">
          <MarkdownDocument
            content={content!}
            location={{ kind: 'repo', repoId, path, ref: gitRef ?? undefined, changeId }}
            onClick={(e) => handlePreviewLinkClick(e, encodeRepoPath(locator), locator)}
          />
        </div>
      );
    }
    if (body === 'csv') return <div class="repo-file-rendered" dangerouslySetInnerHTML={{ __html: csvHtml! }} />;

    return (
      <div class="repo-file-content">
        <LineNumberedCode rows={rows} wideLines={filePreviewWrap.value ? 'wrap' : 'pan'} />
      </div>
    );
  }
}
