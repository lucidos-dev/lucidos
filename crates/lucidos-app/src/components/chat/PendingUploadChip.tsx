import type { PendingUpload, PendingUploadState } from '../../store/pendingUploads';
import { BlobImage } from '../shared/BlobImage';
import { CloseIcon, RetryIcon, WarningIcon } from '../shared/icons';

export interface PendingChipView {
  ring: number | 'spin' | null;
  caption: string;
  label: string;
  action: 'none' | 'retry' | 'refused';
}

export function pendingChipView(state: PendingUploadState): PendingChipView {
  switch (state.kind) {
    case 'waiting-for-thread':
      return { ring: 'spin', caption: 'Waiting', label: 'Waiting for Lucidos to start this draft', action: 'none' };
    case 'uploading': {
      const fraction = state.totalBytes > 0 ? Math.min(state.sentBytes / state.totalBytes, 1) : 0;
      const percent = Math.round(fraction * 100);
      return { ring: fraction, caption: `${percent}%`, label: `Uploading image, ${percent}%`, action: 'none' };
    }
    case 'finishing':
      return { ring: 'spin', caption: 'Uploading', label: 'Uploading image, waiting for Lucidos to confirm', action: 'none' };
    case 'retrying':
      return { ring: 'spin', caption: 'Retrying', label: `Retrying the upload (try ${state.attempt}): ${state.reason}`, action: 'none' };
    case 'offline':
      return { ring: 'spin', caption: 'Offline', label: `Waiting for the connection: ${state.reason}`, action: 'none' };
    case 'failed':
      return state.retryable
        ? { ring: null, caption: 'Retry', label: `Upload failed: ${state.reason}. Retry`, action: 'retry' }
        : { ring: null, caption: 'Failed', label: `Upload failed: ${state.reason}`, action: 'refused' };
  }
}

const RING_RADIUS = 15;
const RING_CIRCUMFERENCE = 2 * Math.PI * RING_RADIUS;

function ProgressRing({ ring }: { ring: number | 'spin' }) {
  const fraction = ring === 'spin' ? 0.25 : ring;
  return (
    <svg class={`upload-ring${ring === 'spin' ? ' upload-ring-spin' : ''}`} viewBox="0 0 36 36" aria-hidden="true">
      <circle class="upload-ring-track" cx="18" cy="18" r={RING_RADIUS} />
      <circle
        class="upload-ring-bar"
        cx="18"
        cy="18"
        r={RING_RADIUS}
        transform="rotate(-90 18 18)"
        stroke-dasharray={RING_CIRCUMFERENCE}
        stroke-dashoffset={RING_CIRCUMFERENCE * (1 - fraction)}
      />
    </svg>
  );
}

interface PendingUploadChipProps {
  upload: PendingUpload;
  onOpen: (img: HTMLImageElement) => void;
  onRetry: () => void;
  onRemove: () => void;
}

export function PendingUploadChip({ upload, onOpen, onRetry, onRemove }: PendingUploadChipProps) {
  const view = pendingChipView(upload.state);
  const failed = upload.state.kind === 'failed';
  return (
    <div
      class={`image-preview-item image-preview-pending${failed ? ' image-preview-pending-failed' : ''}`}
      data-upload-state={upload.state.kind}
    >
      <BlobImage
        src={upload.previewUrl}
        class="image-preview-thumb"
        onClick={(e) => onOpen(e.currentTarget)}
        data-tooltip={view.action === 'none' ? view.label : undefined}
      />
      {view.action === 'retry' ? (
        <button
          type="button"
          class="upload-overlay upload-retry"
          onClick={onRetry}
          aria-label={view.label}
          data-tooltip={view.label}
        >
          <RetryIcon />
          <span class="upload-caption">{view.caption}</span>
        </button>
      ) : view.action === 'refused' ? (
        <div class="upload-overlay upload-refused" role="img" aria-label={view.label} data-tooltip={view.label}>
          <WarningIcon />
          <span class="upload-caption">{view.caption}</span>
        </div>
      ) : (
        <div
          class="upload-overlay"
          role="progressbar"
          aria-label={view.label}
          aria-valuemin={0}
          aria-valuemax={100}
          aria-valuenow={typeof view.ring === 'number' ? Math.round(view.ring * 100) : undefined}
        >
          {view.ring !== null && <ProgressRing ring={view.ring} />}
          <span class="upload-caption">{view.caption}</span>
        </div>
      )}
      <button
        type="button"
        class="icon-btn image-preview-remove"
        onClick={onRemove}
        aria-label={failed ? 'Remove failed upload' : 'Cancel upload'}
        data-tooltip={failed ? 'Remove' : 'Cancel'}
      ><CloseIcon /></button>
    </div>
  );
}
