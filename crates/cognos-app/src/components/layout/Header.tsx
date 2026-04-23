import { uploadProgress } from '../../store/store';
import { CloseIcon } from '../shared/icons';

function dismissUpload() {
  uploadProgress.value = null;
}

export function UploadIndicatorBar() {
  const p = uploadProgress.value;
  if (!p) return null;

  if (p.status === 'uploading') {
    const label = p.total === 1
      ? `Importing ${p.filename}`
      : `Importing ${p.current}/${p.total}: ${p.filename}`;
    return (
      <div class="upload-indicator">
        <span class="upload-dot"></span>
        {label}
      </div>
    );
  }

  if (p.failed === 0) {
    const label = p.succeeded === 1
      ? '1 file successfully imported'
      : `${p.succeeded} files successfully imported`;
    return <div class="upload-indicator">{label} 🎉</div>;
  }

  const errorDetail = p.errors.length > 0 ? p.errors.join('; ') : 'Unknown error';
  const label = p.succeeded > 0
    ? `${p.succeeded} imported, ${p.failed} failed`
    : p.failed === 1 ? 'Import failed' : `${p.failed} imports failed`;

  return (
    <div class="upload-indicator upload-error">
      <span>{label} — {errorDetail}</span>
      <button class="icon-btn upload-dismiss" onClick={dismissUpload} aria-label="Dismiss" data-tooltip="Dismiss"><CloseIcon /></button>
    </div>
  );
}
