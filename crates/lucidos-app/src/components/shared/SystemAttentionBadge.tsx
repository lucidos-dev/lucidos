/**
 * The *System attention badge*, drawn wherever the path into Settings > System
 * passes: the menu-drawer button, that drawer's Settings row, the Settings
 * home's System row, and the two System tabs that can owe something.
 *
 * It renders nothing when the workspace owes nothing, and nothing holds its
 * space, so no row moves as it comes and goes. The rule and the words are in
 * `store/systemAttentionBadge.ts`.
 *
 * **Each host passes the read it wants.** Upstream of the tabs one mark stands
 * for both causes, so those hosts pass the union. A TAB passes its own half.
 *
 * **The mark is decorative, and the HOST says the words.** Off-screen text here
 * would land in `textContent`, fused with the label ("Settings1 thing to do").
 * So each host that can name itself composes the sentence into its
 * `aria-label`. The drawer's Settings row cannot: it is a role-less `<div>`.
 *
 * `placement` picks the geometry. `corner` takes `.badge` with it.
 */
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
