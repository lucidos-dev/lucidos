import type { PendingUpload, PendingUploadState } from '../../store/pendingUploads';
import { BlobImage } from '../shared/BlobImage';
import { CloseIcon, RetryIcon, WarningIcon } from '../shared/icons';

/** How one pending chip draws its state. `ring` is a fraction for a known
 *  amount and `'spin'` for a wait of unknown length. It is null when the chip
 *  shows an action or a verdict instead. */
export interface PendingChipView {
  ring: number | 'spin' | null;
  caption: string;
  /** The accessible name of the state, and its tooltip. */
  label: string;
  /** What the overlay offers: nothing, a Retry, or a refusal that cannot be retried. */
  action: 'none' | 'retry' | 'refused';
}

/** Pure, so every state is pinned by a test without a renderer. */
export function pendingChipView(state: PendingUploadState): PendingChipView {
  switch (state.kind) {
    case 'waiting-for-thread':
      return { ring: 'spin', caption: 'Waiting', label: 'Waiting for Lucidos to start this draft', action: 'none' };
    case 'uploading': {
      const fraction = state.totalBytes > 0 ? Math.min(state.sentBytes / state.totalBytes, 1) : 0;
      const percent = Math.round(fraction * 100);
      return { ring: fraction, caption: `${percent}%`, label: `Uploading image, ${percent}%`, action: 'none' };
    }
    // Still uploading to the user: on a phone, WebKit hands over a small image
    // before any of its bytes travel, so this state covers the real transfer.
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
  // A spinning ring shows a fixed quarter arc; the turning says "working".
  const fraction = ring === 'spin' ? 0.25 : ring;
  return (
    <svg class={`upload-ring${ring === 'spin' ? ' upload-ring-spin' : ''}`} viewBox="0 0 36 36" aria-hidden="true">
      <circle class="upload-ring-track" cx="18" cy="18" r={RING_RADIUS} />
      <circle
        class="upload-ring-bar"
        cx="18"
        cy="18"
        r={RING_RADIUS}
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

/** An attached image still on its way to the engine. It never looks like a
 *  finished one: it shows progress, a wait, or what went wrong. */
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
        // The progress overlay lets taps through to the thumbnail, so the
        // thumbnail carries its tooltip.
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
