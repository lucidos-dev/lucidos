import { useEffect, useRef } from 'preact/hooks';
import { useSignalEffect } from '@preact/signals';
import type { VNode } from 'preact';
import { filePreviewModal, panelOverlay, parseRepoPath, type PanelOverlay } from '../../store/store';
import { closeFilePreviewModal, escalateFilePreviewModal } from '../../store/actions/filePreviewModal';
import { useHidePanelWebviewWhile } from '../../hooks/useHidePanelWebviewWhile';
import { viewportIsMobile } from '../../utils/viewport';
import { Overlay } from '../shared/Overlay';
import { CloseIcon } from '../shared/icons';
import { trapDialogTab } from '../shared/dialogFocusTrap';
import { FilePreviewInline } from './FilePreviewInline';
import { RepoFileContent, previewGitRef } from './RepoFilePreview';
import { previewFilePath, previewFileName } from '../../utils/previewPath';

type LineRange = { start: number; end: number } | null;

/** How the modal names the file it is showing: the basename with the cited line
 *  appended the way a citation writes it (`main.rs:510-520`), plus the full path
 *  underneath. A repo locator is named by its repo-relative path, never by the
 *  raw encoding, which would read as uuid soup.
 *
 *  Both halves come from `utils/previewPath`, the one place a preview locator is
 *  turned into something displayable, so this modal, the content header's title
 *  and the preview's own path row all name the same file the same way.
 *
 *  Pure and exported so the naming is testable without a DOM. */
export function filePreviewModalTitle(path: string, range: LineRange): { name: string; detail: string } {
  const lines = range === null
    ? ''
    : range.end > range.start ? `:${range.start}-${range.end}` : `:${range.start}`;
  return { name: `${previewFileName(path)}${lines}`, detail: previewFilePath(path) };
}

/** Which preview renders for a locator: the registered-repository one for a
 *  `repo:` path, the workspace-data one otherwise. Both are the Files panel's
 *  own components, so the highlight, the line numbers and the
 *  source-vs-rendered behaviour are the ones the panel shows.
 *
 *  Two things decide which revision of a repo file is shown, and neither is the
 *  Files panel's state: the modal may be previewing a repository the panel is
 *  not bound to, so the panel's pending coding-agent branch is not this file's
 *  revision.
 *
 *    - A `diff` locator renders the file at its CHANGE (`changeId`), which
 *      `RepoFileContent` fetches through /api/v1/changes/:id/file: the end
 *      state, correct for a pending branch and an applied post-merge sha alike.
 *      The modal is a file preview, so the diff locator names the file's
 *      revision rather than asking for hunks (see `openFilePreviewModal`).
 *    - A `file` locator renders at the ref it names, or at `HEAD` when it names
 *      none (`previewGitRef` with a `null` surface default).
 *
 *  Pure and exported so the choice is testable without a DOM. */
export function filePreviewModalBody(path: string, layout: 'desktop' | 'mobile'): VNode {
  const repo = parseRepoPath(path);
  if (!repo) return <FilePreviewInline path={path} layout={layout} />;
  return (
    <RepoFileContent
      repoId={repo.repoId}
      path={repo.path}
      changeId={repo.mode === 'diff' ? repo.changeId : undefined}
      gitRef={previewGitRef(repo, null)}
    />
  );
}

/** A read-only glance at a file, over whatever the content pane is showing.
 *
 *  Opened by an app through `lucidos.ui.previewFile` so a reader following a
 *  citation in a report does not lose their place: the shell does not navigate,
 *  and the app is still there when the modal closes. The escalation in the
 *  header promotes the glance into the real thing, the same Files preview
 *  `lucidos.ui.navigate('file', …)` would have opened.
 *
 *  Mounted from `App` only while a preview is showing (see
 *  `FilePreviewModalSlot`), which also keeps the preview renderers out of the
 *  initial bundle. */
export function FilePreviewModal() {
  const state = filePreviewModal.value;
  const panelRef = useRef<HTMLDivElement>(null);
  const closeRef = useRef<HTMLButtonElement>(null);
  // `undefined` means "not captured yet"; `null` is a legitimate overlay value
  // (no panel overlay), so the two must stay distinguishable.
  const openedOver = useRef<PanelOverlay | undefined>(undefined);

  // The native panel webview paints over HTML; hold it hidden while open.
  useHidePanelWebviewWhile(state !== null);

  // Capture what the content pane was showing when this preview opened, and put
  // the keyboard on the close control. Keyed on the open id so a second
  // previewFile replacing this one re-captures and re-focuses.
  useEffect(() => {
    if (!state) { openedOver.current = undefined; return; }
    openedOver.current = panelOverlay.peek();
    closeRef.current?.focus();
  }, [state?.id]);

  // A link inside the previewed document can route the shell out from under the
  // glance: a markdown artifact's sibling link goes through
  // `handlePreviewLinkClick`, and the knowhow "did you mean" suggestion calls
  // `openFilePreview`. Either would leave the modal hanging over a pane that has
  // moved on, so close when the content pane changes. Declared after the capture
  // effect so its first run (effects fire in declaration order) already sees the
  // captured value and cannot self-close.
  //
  // `navigated` is what stops the close from clobbering where we just landed:
  // the opener configures the destination's view state BEFORE it sets
  // `panelOverlay`, so by the time this fires the borrowed state is already the
  // destination's, and handing the snapshot back would overwrite it.
  useSignalEffect(() => {
    const showing = panelOverlay.value;
    if (filePreviewModal.peek() === null) return;
    if (openedOver.current === undefined) return;
    if (showing !== openedOver.current) closeFilePreviewModal({ navigated: true });
  });

  // Keep Tab inside the modal, same as the confirm/prompt dialogs.
  const open = state !== null;
  useEffect(() => {
    if (!open) return;
    const onKey = (e: KeyboardEvent) => trapDialogTab(e, panelRef.current);
    document.addEventListener('keydown', onKey);
    return () => document.removeEventListener('keydown', onKey);
  }, [open]);

  if (!state) return null;

  const { name, detail } = filePreviewModalTitle(state.path, state.range);
  const layout = viewportIsMobile.value ? 'mobile' : 'desktop';

  return (
    <Overlay
      open
      // Arrow-wrapped, not passed by reference: `closeFilePreviewModal` takes an
      // options object, and a handler bound directly would hand it the DOM event
      // as those options.
      onClose={() => closeFilePreviewModal()}
      panelClass="file-preview-modal"
      panelRole="dialog"
      ariaModal
      dataRole="file-preview-modal"
      panelRef={panelRef}
    >
      <div class="file-preview-modal-header">
        <div class="file-preview-modal-title">
          <span class="file-preview-modal-name">{name}</span>
          <span class="file-preview-modal-detail">{detail}</span>
        </div>
        <div class="file-preview-modal-actions">
          <button class="accent-link" onClick={escalateFilePreviewModal}>
            Open in Files
          </button>
          <button
            ref={closeRef}
            class="icon-btn"
            aria-label="Close preview"
            data-tooltip="Close preview"
            onClick={() => closeFilePreviewModal()}
          >
            <CloseIcon />
          </button>
        </div>
      </div>
      {/* A declared scroll region, mirroring `.content-pane-body`. The modal's
          Tab trap cycles its own controls, and `tabIndex={0}` puts the body
          among them. So a keyboard user reaches it and scrolls a long preview.
          Chrome gave that for free by promoting an overflowing scroller, but
          only while the preview holds no link, and it arrived unnamed wearing
          the browser's own ring. Declaring the stop makes it every browser's
          and names it, and components.css gives it our inset ring in place of
          the browser's. */}
      <div
        class="file-preview-modal-body"
        tabIndex={0}
        role="region"
        aria-label="File preview"
      >
        {filePreviewModalBody(state.path, layout)}
      </div>
    </Overlay>
  );
}
