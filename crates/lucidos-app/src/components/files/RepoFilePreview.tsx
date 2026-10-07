import { useEffect, useMemo, useState } from 'preact/hooks';
import type { DiffFile, RepoDiff, RepoLocator } from '../../store/store';
import { repoDiff, repoPending, filePreviewSource, filePreviewWrap, diffSideBySide, repoSelectedChangeId, openFilePreviewRevision, encodeRepoPath } from '../../store/store';
import { diffBodyKind } from '../../store/diffBody';
import type { Loadable } from '../../store/types';
import { getRepoFileContent, getChangeFileContent, repoFileUrl, changeFileUrl } from '../../api/client';
import { loadChangeContextById } from '../../store/actions/repositories';
import { refreshFilePreview, registerPreviewTextBody, reportPreviewTextSettled } from '../../store/actions/artifacts';
import { usePanelRefresh } from '../../hooks/usePanelRefresh';
import { highlightFileLines, CODE_EXTS } from '../../utils/syntaxHighlight';
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
import { IframeTabExit } from '../shared/IframeTabExit';

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

/** Renders RepoFilePreview with a resizable sidebar listing the changed files in
 *  the current diff. A container query hides the sidebar on a narrow content
 *  pane (see `.repo-preview-split-sidebar` in panels/content.css). Mobile and a
 *  heavily-collapsed content pane then show the preview alone. */
export function RepoFilePreviewWithSidebar(props: Props) {
  const isActiveLayout = props.layout === (viewportIsMobile.value ? 'mobile' : 'desktop');
  // Delay the sidebar skeleton (300ms) so a fast diff load never flashes it.
  const showSidebarLoading = useDelayedLoading(repoDiff.value);
  // A diff is pinned to its change, so it has nothing to refresh.
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
  // Resolved once here and passed down rather than read inside the leaves: the
  // same leaves render in the app-facing preview modal, whose surface default is
  // not this one (see `previewGitRef`).
  const gitRef = previewGitRef(locator, repoPending.value?.branch_name ?? null);
  // What the header's Refresh button moves. Read here rather than in the leaves
  // below, which also render in the file preview modal: that surface is not the
  // content pane this revision speaks for, and has no Refresh button of its own.
  const revision = openFilePreviewRevision.value;

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

    // Which of the four bodies shows is derived in the store, because the
    // header derives the same thing to decide which toggles to offer (see
    // `diffBodyKind`). A `null` here means the diff is not loaded, which the
    // guards above have already handled.
    switch (diffBodyKind.value) {
      // The file as it would be once merged (end state), rendered like the All
      // Files view. Added files default to this (their diff is all additions);
      // see diffWholeFileEffective.
      case 'whole-file':
        return <RepoFileContent repoId={repoId} path={path} changeId={activeChangeId ?? undefined} gitRef={gitRef} revision={revision} />;
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
  /** Cache-buster for the header's Refresh button, 0 when it has never been
   *  pressed on this file. A repo file is read at a git ref, so nothing else
   *  moves it: no SSE event names a `repo:` path, and an edit inside a clone
   *  announces nothing. Omitted by the file preview modal, which has no
   *  Refresh button and re-fetches by remounting. */
  revision?: number;
}

/** Dispatches a repo file to the right preview. Media files (images including
 *  a rendered SVG, video, audio, pdf) are pointed at the file URL via a media
 *  element. The engine serves them with a content-type from the extension.
 *  Everything else (source, markdown, csv, and any extensionless or unknown
 *  textual file) goes through the text path, which fetches the body as a
 *  string. Fetching a PNG as text was the bug: it rendered the raw bytes
 *  line-numbered.
 *
 *  Exported because the file preview modal renders the same content over an app
 *  without navigating to the Files panel (see `FilePreviewModal`). It is the
 *  content alone: the panel's chrome (the changed-files sidebar, the diff modes)
 *  stays with `RepoFilePreviewWithSidebar`. */
export function RepoFileContent({ repoId, path, changeId, gitRef, revision }: RepoFileContentProps) {
  const body = repoPreviewBody(path, { sourceToggle: filePreviewSource.value });
  if (body === 'image' || body === 'pdf' || body === 'video' || body === 'audio') {
    return <RepoFileMedia repoId={repoId} path={path} changeId={changeId} gitRef={gitRef} revision={revision} kind={body} />;
  }
  return <RepoFileText key={`${repoId}:${changeId ?? ''}:${gitRef ?? ''}:${path}`} repoId={repoId} path={path} changeId={changeId} gitRef={gitRef} revision={revision} body={body} />;
}

/** Binary-media preview. Builds the file URL (same change-vs-branch ref logic as
 *  RepoFileText) and renders it without fetching the bytes as text. */
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

/** A repository image, or a load error when it cannot be read: a `repo:` link
 *  can name a file or ref that does not exist. Keyed by `src` at the call site,
 *  so the next file starts without the last one's failure. */
function RepoImage({ src, path }: { src: string; path: string }) {
  const [failed, setFailed] = useState(false);
  if (failed) return <LoadableError noun="image" error={`${path} could not be read from the repository`} />;
  return <PreviewImage src={src} alt={path} onError={() => setFailed(true)} />;
}

function RepoFileText({ repoId, path, changeId, gitRef, revision, body }: RepoFileContentProps & { body: 'markdown' | 'csv' | 'source' }) {
  // With a Lucidos/app change row, fetch the end state via /changes/:id/file —
  // the correct ref for both pending (branch) and applied (post_merge_sha). Without
  // one (external-repo CC), fall back to the branch ref. Mirrors RenderedDiff.
  //
  // `revision` is in the deps rather than in the URL: re-running the fetch is
  // the whole job here, and the engine answers `Cache-Control: no-cache`, so a
  // plain re-request already revalidates. The media branch above cache-busts
  // instead, because an element keeps its bytes until its `src` changes.
  //
  // Only the content pane passes a `revision`, so only there does this body
  // hold a panel refresh open until its re-read lands. The caller keys it by
  // file, which is what makes keeping the old text on screen safe.
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
    [content, ext, isCode],
  );

  if (loadable.status === 'failed') return <LoadableError noun="file" error={loadable.error} />;
  // The body type is known before the read lands, so the placeholder takes
  // the shape of what will replace it.
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
    // There is no html body here. `REPO_RENDERABLE_EXTS` excludes it, so a repo
    // HTML file renders as syntax-highlighted source. A live srcDoc iframe would
    // show the app shell's boot splash instead of the file.
    //
    // `.repo-file-rendered` insets the content to match the rendered diff
    // (.rendered-diff), so toggling between them keeps the same gutter.
    if (body === 'markdown') {
      // Shared by the Files-panel preview AND the app-facing glance modal
      // (both render through `RepoFileContent` → here), so a sibling link
      // routes identically from either surface.
      //
      // Always `mode: 'file'`, even while THIS file is shown via a change's
      // whole-file diff view (`changeId` set): a sibling link names a plain
      // file, not another diff entry, and a sibling outside the change has
      // no diff entry at all. The `whole-file` case above already passes
      // this file's own `gitRef` as the change's branch, so the sibling
      // still lands on that same branch.
      //
      // Its images, though, are read where the document's own text was read:
      // from the change when one is set, which follows it once applied.
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
