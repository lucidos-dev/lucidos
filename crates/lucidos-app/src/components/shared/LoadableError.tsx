interface Props {
  error: string;
  noun: string;
}

/** `<div class="empty-state error-text">Failed to load {noun}: {error}</div>` —
 *  use inside the consumer's `status === 'failed'` branch. */
export function LoadableError({ error, noun }: Props) {
  return <div class="empty-state error-text">Failed to load {noun}: {error}</div>;
}
