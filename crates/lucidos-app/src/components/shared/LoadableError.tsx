interface Props {
  error: string;
  noun: string;
  /** Offers a Retry beside the message. For a read the user needs before they
   *  can do anything else, where the failure is often just a slow machine. */
  onRetry?: () => void;
}

/** `<div class="empty-state error-text">Failed to load {noun}: {error}</div>` —
 *  use inside the consumer's `status === 'failed'` branch. */
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
