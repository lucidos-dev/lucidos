export function SystemAttentionBadge(
  { placement, label }: { placement: 'corner' | 'inline'; label: string | null },
) {
  if (!label) return null;
  if (placement === 'corner') {
    return <span class="badge system-attention-badge-corner" aria-hidden="true" />;
  }
  return (
    <span class="system-attention-badge-slot" aria-hidden="true">
      <span class="system-attention-badge" />
    </span>
  );
}
