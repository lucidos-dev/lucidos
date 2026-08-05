import { useEffect, useMemo } from 'preact/hooks';
import type { DiffFile, RepoDiff, RepoLocator } from '../../store/store';
import { repoDiff, repoPending, filePreviewSource, diffSideBySide, openImagePopup, repoSelectedChangeId } from '../../store/store';
import { diffBodyKind } from '../../store/diffBody';
import type { Loadable } from '../../store/types';
import { getRepoFileContent, getChangeFileContent, repoFileUrl, changeFileUrl } from '../../api/client';
import { loadChangeContextById } from '../../store/actions/repositories';
import { highlightFileLines, CODE_EXTS } from '../../utils/syntaxHighlight';
import { escapeHtml } from '../../utils/escapeHtml';
import { renderMarkdown } from '../../utils/renderMarkdown';
import { renderCsvTable } from '../../utils/csv';
import { REPO_RENDERABLE_EXTS, previewMediaKind } from './previewExts';
import { isMobile, viewportIsMobile } from '../../utils/viewport';
import { useLoadableFetch } from '../../hooks/useLoadableFetch';
import { useDelayedLoading } from '../../hooks/useDelayedLoading';
import { DiffView } from './DiffView';
import { RenderedDiff } from './RenderedDiff';
import { ChangesFileList } from './RepoFilesView';
import { LoadableError } from '../shared/LoadableError';
import { LineNumberedCode, fileRows } from './LineNumberedCode';
import { bridgePreviewIframeShortcuts } from './previewIframeShortcuts';

interface Props {
  /** The parsed `repo:` locator the panel overlay holds. Its per-mode qualifier
   *  is what the preview needs beyond the path: for a diff, the change to fetch
   *  when the overlay was restored from nav history after a reload (repoDiff is
   *  runtime-only, so it is empty then); for a file, the git revision to read it
   *  at. */
  locator: RepoLocator;
  /** Skip mounting in the inactive dual-rendered layout — otherwise both
   *  SplitLayout and MobileSwipeContainer copies fetch and decode the file. */
  layout: 'desktop' | 'mobile';
}

/** Which git revision a locator's file is read at.
 *
 *  The locator's own ref always wins: `repo:<id>:file#<ref>:<path>` is a caller
 *  saying which revision it cited, and that caller (an app, an LLM
 *  `navigate_ui`, an `<a href>` in a report) knows something the UI does not.
 *
 *  `surfaceDefault` is what to read when the locator names no ref, and it is
 *  deliberately the SURFACE's business rather than this function's, because the
 *  two surfaces have genuinely different answers:
 *
 *    - The Files panel is bound to one repository, so its default is that
 *      repository's pending coding-agent branch: the revision the user is
 *      already looking at.
 *    - The preview modal may be showing a repository the panel is NOT bound to,
 *      so the panel's branch would be the wrong repository's. Its default is
 *      `null`, the clone's `HEAD`, which is what a bare `repo:` locator means.
 *
 *  A `diff` locator carries a change id rather than a ref, so it always takes
 *  the surface default. */
export function previewGitRef(locator: RepoLocator, surfaceDefault: string | null): string | null {
  return (locator.mode === 'file' ? locator.ref : undefined) ?? surfaceDefault;
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
  // Delay the sidebar skeleton (300ms) so a fast diff load never flashes it.
  const showSidebarLoading = useDelayedLoading(repoDiff.value);
  if (!isActiveLayout) return null;

  const sidebar = sidebarStateFromDiff(repoDiff.value);

  if (sidebar.kind === 'hidden') {
    return <RepoFilePreview {...props} />;
  }

  return (
    <div class="repo-preview-split">
      <aside class="repo-preview-split-sidebar">
        {sidebar.kind === 'loading' && showSidebarLoading && (
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
          <ChangesFileList files={sidebar.files} activePath={props.locator.path} />
        )}
      </aside>
      <div class="repo-preview-split-main">
        <RepoFilePreview {...props} />
      </div>
    </div>
  );
}

function RepoFilePreview({ locator, layout }: Props) {
  const { repoId, mode, path } = locator;
  const changeId = locator.mode === 'diff' ? locator.changeId : undefined;
  const isActiveLayout = layout === (viewportIsMobile.value ? 'mobile' : 'desktop');
  const showDiffLoading = useDelayedLoading(repoDiff.value);
  // Resolved once here and passed down rather than read inside the leaves: the
  // same leaves render in the app-facing preview modal, whose surface default is
  // not this one (see `previewGitRef`).
  const gitRef = previewGitRef(locator, repoPending.value?.branch_name ?? null);

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

    const activeChangeId = changeId ?? repoSelectedChangeId.value;

    // Which of the four bodies shows is derived in the store, because the
    // header derives the same thing to decide which toggles to offer (see
    // `diffBodyKind`). A `null` here means the diff is not loaded, which the
    // guards above have already handled.
    switch (diffBodyKind.value) {
      // The file as it would be once merged (end state), rendered like the All
      // Files view. Added files default to this (their diff is all additions);
      // see diffWholeFileEffective.
      case 'whole-file':
        return <RepoFileContent repoId={repoId} path={path} changeId={activeChangeId ?? undefined} gitRef={gitRef} />;
      case 'no-end-state':
        return <div class="empty-state">File is deleted in this change, so there is no end state to show</div>;
      case 'rendered-markdown':
        return <RenderedDiff file={file} changeId={activeChangeId} repoId={repoId} gitRef={gitRef} />;
      default:
        // The one instance that measures: this is the diff the content-pane
        // header's Side by side toggle acts on (see `measureFit`).
        return <DiffView file={file} sideBySide={diffSideBySide.value} measureFit />;
    }
  }

  return <RepoFileContent repoId={repoId} path={path} gitRef={gitRef} />;
}

interface RepoFileContentProps {
  repoId: string;
  path: string;
  changeId?: string;
  /** Which revision of the file to read: a branch name, or `null` for the
   *  clone's current `HEAD`. Always explicit, never read from the bound
   *  repository's state, because this component also renders for a repository
   *  the Files panel is not bound to (the app-facing file preview modal), where
   *  the bound repository's branch would fetch the wrong revision or 404. */
  gitRef: string | null;
}

/** Dispatches a repo file to the right preview. Binary-media files (images,
 *  video, audio, pdf) are pointed at the file URL via a media element — the
 *  engine serves them with a content-type from the extension. Everything else
 *  (source, markdown, csv, svg, and any extensionless/unknown-but-textual file)
 *  goes through the text path, which fetches the body as a string. Fetching a
 *  PNG as text was the bug: it rendered the raw bytes line-numbered.
 *
 *  Exported because the file preview modal renders the same content over an app
 *  without navigating to the Files panel (see `FilePreviewModal`). It is the
 *  content alone: the panel's chrome (the changed-files sidebar, the diff modes)
 *  stays with `RepoFilePreviewWithSidebar`. */
export function RepoFileContent({ repoId, path, changeId, gitRef }: RepoFileContentProps) {
  const ext = path.split('.').pop()?.toLowerCase() || '';
  if (previewMediaKind(ext) !== 'text') {
    return <RepoFileMedia repoId={repoId} path={path} changeId={changeId} gitRef={gitRef} ext={ext} />;
  }
  return <RepoFileText repoId={repoId} path={path} changeId={changeId} gitRef={gitRef} />;
}

/** Binary-media preview. Builds the file URL (same change-vs-branch ref logic as
 *  RepoFileText) and renders it without fetching the bytes as text. */
function RepoFileMedia({ repoId, path, changeId, gitRef, ext }: RepoFileContentProps & { ext: string }) {
  const url = changeId ? changeFileUrl(changeId, path) : repoFileUrl(repoId, path, gitRef ?? undefined);
  const kind = previewMediaKind(ext);

  if (kind === 'image') {
    return <img src={url} alt={path} style="max-width:100%;max-height:100%;object-fit:contain;" onClick={() => { if (isMobile()) openImagePopup(url); }} />;
  }
  if (kind === 'pdf') return <iframe src={url} style="width:100%;height:100%;border:none;" onLoad={(e) => bridgePreviewIframeShortcuts(e.currentTarget)} />;
  if (kind === 'video') return <video src={url} controls style="max-width:100%;max-height:100%;" />;
  return <audio src={url} controls style="width:100%;" />;
}

function RepoFileText({ repoId, path, changeId, gitRef }: RepoFileContentProps) {
  // With a Lucidos/app change row, fetch the end state via /changes/:id/file —
  // the correct ref for both pending (branch) and applied (post_merge_sha). Without
  // one (external-repo CC), fall back to the branch ref. Mirrors RenderedDiff.
  const { loadable, showLoading } = useLoadableFetch<string>(
    () => changeId
      ? getChangeFileContent(changeId, path)
      : getRepoFileContent(repoId, path, gitRef ?? undefined),
    [repoId, path, changeId, gitRef],
  );

  const ext = path.split('.').pop()?.toLowerCase() || '';
  const content = loadable.status === 'loaded' ? loadable.data : null;
  // REPO_RENDERABLE_EXTS (not RENDERABLE_EXTS): repo HTML is source under review,
  // so it falls through to the syntax-highlighted source path below instead of a
  // live srcDoc iframe that would show the app shell's boot splash.
  const renderPreview = content !== null && !filePreviewSource.value && REPO_RENDERABLE_EXTS.includes(ext);
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

  const rows = useMemo(
    () => fileRows(content ? (isCode ? highlightFileLines(content, ext) : content.split('\n').map(escapeHtml)) : []),
    [content, ext, isCode],
  );

  if (loadable.status === 'failed') return <LoadableError noun="file" error={loadable.error} />;
  if (content === null) return showLoading ? <div class="loading-spinner" /> : null;

  if (renderPreview) {
    // No html/htm branch: REPO_RENDERABLE_EXTS excludes them, so a repo HTML file
    // never reaches here — it renders as syntax-highlighted source below.
    // `.repo-file-rendered` insets the content to match the rendered diff
    // (.rendered-diff), so toggling diff ↔ full file keeps the same gutter.
    if (ext === 'md') return <div class="repo-file-rendered"><div class="response-content markdown-content" dangerouslySetInnerHTML={{ __html: renderedHtml! }} /></div>;
    if (ext === 'csv') return <div class="repo-file-rendered" dangerouslySetInnerHTML={{ __html: renderedHtml! }} />;
    // The media variant keeps a definite height so the image's max-height:100%
    // still fits the pane (the bare padding wrapper would leave it unconstrained).
    if (ext === 'svg') return <div class="repo-file-rendered repo-file-rendered-media"><img src={renderedHtml!} alt={path} style="max-width:100%;max-height:100%;object-fit:contain;" onClick={() => { if (isMobile()) openImagePopup(renderedHtml!); }} /></div>;
  }

  return (
    <div class="repo-file-content">
      <LineNumberedCode rows={rows} />
    </div>
  );
}
