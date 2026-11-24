export function progressFillWidth(fraction: number): string {
  if (!Number.isFinite(fraction)) return '0%';
  return `${Math.min(1, Math.max(0, fraction)) * 100}%`;
}

export function ProgressBar({ done, total }: { done: number; total: number }) {
  if (total <= 0) return null;
  return (
    <div class="progress-bar">
      <div class="progress-bar-fill" style={{ width: progressFillWidth(done / total) }} />
    </div>
  );
}
