interface Props {
  error: string;
  noun: string;
  onRetry?: () => void;
}

export function LoadableError({ error, noun, onRetry }: Props) {
  return (
    <div class="empty-state error-text">
      Failed to load {noun}: {error}
      {onRetry && (
        <div>
          <button type="button" class="action-btn" onClick={onRetry}>Retry</button>
        </div>
      )}
    </div>
  );
}
