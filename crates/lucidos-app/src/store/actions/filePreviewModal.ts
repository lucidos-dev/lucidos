import { filePreviewModal, createPreviewViewState } from '../store';
import { describeOverlayTarget, fullscreenBlocksHostOverlays } from '../appFullscreenHost';
import { resolveFileTarget } from './fileTarget';
import { handleNavigationRequest } from './navigation-request';

export interface FilePreviewRequest {
  file_path: string;
  line?: unknown;
  line_end?: unknown;
}

export function filePreviewRequestError(payload: { file_path?: unknown }): string | null {
  if (typeof payload.file_path !== 'string' || payload.file_path.length === 0) {
    return 'previewFile: file_path must be a non-empty string';
  }
  return null;
}

export function filePreviewBlockedReason(
  blocked: boolean = fullscreenBlocksHostOverlays(),
): string | null {
  return blocked
    ? `previewFile: cannot show a preview over a fullscreen element the host does not control (${describeOverlayTarget()})`
    : null;
}

let openCounter = 0;

export function openFilePreviewModal(request: FilePreviewRequest): void {
  const target = resolveFileTarget(request.file_path, request.line, request.line_end, 'file');

  const view = createPreviewViewState();
  view.source.value = target.range !== null;
  view.selectedLines.value = target.range;
  view.lineScrollTarget.value = target.range && { path: target.path, line: target.range.start };

  filePreviewModal.value = { id: ++openCounter, path: target.path, range: target.range, view };
}

export function closeFilePreviewModal(): void {
  filePreviewModal.value = null;
}

export function escalateFilePreviewModal(): void {
  const state = filePreviewModal.peek();
  if (!state) return;
  closeFilePreviewModal();
  handleNavigationRequest({
    target: 'file',
    file_path: state.path,
    line: state.range?.start,
    line_end: state.range?.end,
  });
}
