import { whatsNewBadge } from '../../store/whatsNewBadge';

/**
 * The *What's New badge*, drawn wherever the path into it passes: the
 * menu-drawer button, that drawer's Settings row, the Settings home's System
 * row, and System's What's New tab.
 *
 * It renders nothing at all when the workspace owes nothing, and nothing holds
 * its space, so no row moves as it comes and goes. The rule and the words are
 * in `store/whatsNewBadge.ts`.
 *
 * **The mark is decorative everywhere, and the HOST says the words.** Off-screen
 * text inside the badge would land in `textContent`, where it fuses with the
 * label ("Settings1 thing to do"). So each host that can name itself composes
 * the sentence into its `aria-label`. The menu drawer's Settings row cannot: it
 * is a role-less `<div>`, and the button that opened it has already spoken.
 *
 * `placement` picks the geometry. `corner` takes `.badge` with it, so the
 * header bar's own repaint and ring apply and the dot cannot fuse with the
 * glyph under it.
 */
export function WhatsNewBadge({ placement }: { placement: 'corner' | 'inline' }) {
  if (!whatsNewBadge()) return null;
  if (placement === 'corner') {
    return <span class="badge whats-new-badge-corner" aria-hidden="true" />;
  }
  return (
    <span class="whats-new-badge-slot" aria-hidden="true">
      <span class="whats-new-badge" />
    </span>
  );
}
