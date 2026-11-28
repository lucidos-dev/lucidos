import { useEffect, useRef } from 'preact/hooks';
import { useSignalEffect } from '@preact/signals';
import type { VNode } from 'preact';
import { filePreviewModal, filePreviewWrap, panelOverlay, parseRepoPath, type PanelOverlay } from '../../store/store';
import { previewShowsSource } from '../../store/previewWrap';
import { closeFilePreviewModal, escalateFilePreviewModal } from '../../store/actions/filePreviewModal';
import { useHidePanelWebviewWhile } from '../../hooks/useHidePanelWebviewWhile';
import { viewportIsMobile } from '../../utils/viewport';
import { Overlay } from '../shared/Overlay';
import { WrapTextIcon } from '../shared/icons';
import { SurfaceHead } from '../shared/Surface';
import { ToggleIconButton } from '../shared/ToggleIconButton';
import { RepoFileContent, previewGitRef } from './RepoFilePreview';
import { previewFilePath, previewFileName } from '../../utils/previewPath';
import { PreviewPathContext, PreviewViewStateContext } from './previewViewState';
import { filePreviewCopyPathAction } from '../layout/ContentHeaderActions';

let FilePreviewInline: typeof import('./FilePreviewInline')['FilePreviewInline'];

export async function loadFilePreviewModal() {
  ({ FilePreviewInline } = await import('./FilePreviewInline'));
  return FilePreviewModal;
}

type LineRange = { start: number; end: number } | null;

export function filePreviewModalTitle(path: string, range: LineRange): { name: string; detail: string } {
  const lines = range === null
    ? ''
    : range.end > range.start ? `:${range.start}-${range.end}` : `:${range.start}`;
  return { name: `${previewFileName(path)}${lines}`, detail: previewFilePath(path) };
}

export function filePreviewModalBody(path: string, layout: 'desktop' | 'mobile'): VNode {
  const repo = parseRepoPath(path);
  if (!repo) return <FilePreviewInline path={path} layout={layout} modal />;
  return (
    <RepoFileContent
      repoId={repo.repoId}
      path={repo.path}
      changeId={repo.mode === 'diff' ? repo.changeId : undefined}
      gitRef={previewGitRef(repo, null)}
    />
  );
}

export function FilePreviewModal() {
  const state = filePreviewModal.value;
  const panelRef = useRef<HTMLDivElement>(null);
  const openedOver = useRef<PanelOverlay | undefined>(undefined);

  useHidePanelWebviewWhile(state !== null);

  useEffect(() => {
    if (!state) { openedOver.current = undefined; return; }
    openedOver.current = panelOverlay.peek();
    panelRef.current?.querySelector<HTMLButtonElement>('[data-role="surface-close"]')?.focus();
  }, [state?.id]);

  useSignalEffect(() => {
    const showing = panelOverlay.value;
    if (filePreviewModal.peek() === null) return;
    if (openedOver.current === undefined) return;
    if (showing !== openedOver.current) closeFilePreviewModal();
  });

  if (!state) return null;

  const { name, detail } = filePreviewModalTitle(state.path, state.range);
  const layout = viewportIsMobile.value ? 'mobile' : 'desktop';
  const wrapOn = filePreviewWrap.value;
  const canWrap = previewShowsSource(state.path, {
    sourceToggle: state.view.source.value,
    editing: false,
    diffShowsWholeFile: true,
  });
  const copyPath = filePreviewCopyPathAction(state.path);

  return (
    <Overlay
      open
      onClose={closeFilePreviewModal}
      panelClass="surface surface-raised file-preview-modal"
      panelRole="dialog"
      ariaModal
      dataRole="file-preview-modal"
      panelRef={panelRef}
    >
      <SurfaceHead
        title={
          <>
            <span class="file-preview-modal-name">{name}</span>
            <span class="file-preview-modal-detail">{detail}</span>
          </>
        }
        actions={
          <>
            {copyPath && (
              <button
                class="icon-btn"
                aria-label={copyPath.label}
                data-tooltip={copyPath.label}
                onClick={copyPath.onClick}
              >
                {copyPath.icon()}
              </button>
            )}
            {canWrap && (
              <ToggleIconButton
                class="file-preview-wrap-toggle"
                pressed={wrapOn}
                aria-label={wrapOn ? 'Stop wrapping long lines' : 'Wrap long lines'}
                data-tooltip={wrapOn ? 'Stop wrapping long lines' : 'Wrap long lines'}
                onClick={() => { filePreviewWrap.value = !wrapOn; }}
              >
                <WrapTextIcon />
              </ToggleIconButton>
            )}
            <button class="accent-link" onClick={escalateFilePreviewModal}>
              Open in Files
            </button>
          </>
        }
        onClose={closeFilePreviewModal}
        closeLabel="Close preview"
      />
      <div
        class="file-preview-modal-body"
        tabIndex={0}
        role="region"
        aria-label="File preview"
      >
        <PreviewViewStateContext.Provider value={state.view}>
          <PreviewPathContext.Provider value={state.path}>
            {filePreviewModalBody(state.path, layout)}
          </PreviewPathContext.Provider>
        </PreviewViewStateContext.Provider>
      </div>
    </Overlay>
  );
}
