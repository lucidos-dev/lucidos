import { useState } from 'preact/hooks';
import { connectionStatus, visibleWorkspaceName } from '../../store/store';
import { crossWorkspaceUnreadTotal, refreshOtherWorkspacesUnread } from '../../store/actions/app-badge';
import { connectionNotice, connectionNoticeSentence, connectionPhrase } from '../../utils/connectionNotice';
import { Overlay } from '../shared/Overlay';
import { LucidosMarkIcon } from '../shared/icons';
import { BrandBadge, UnreadBrandBadge, brandBadgeLabel, brandTooltipRows, unreadBadgeLabel } from './BrandBadge';
import { liveActivityRows } from '../../store/actions/activityRows';
import { lazyComponent } from '../../utils/lazyComponent';
import { usePendingOpen } from '../../hooks/usePendingOpen';
import { prefetchWhenIdle } from '../../utils/idlePrefetch';

/** The menu's rows, fetched once the boot splash lifts (ADR 0288). The menu
 *  opens only once they are in memory, so no row pops in. */
const BrandMenuRows = lazyComponent(() => import('./BrandMenuRows').then((m) => m.BrandMenuRows));
prefetchWhenIdle(BrandMenuRows);

export const MENU_NAME = 'Lucidos menu';

/**
 * The Lucidos menu: Home, the work in flight, New thread, Search everywhere,
 * Workspaces, Refresh.
 *
 * A CENTRED MODAL over a dimmed app, not a panel hanging off whatever opened
 * it. Three hosts open it (the mobile thread pane's centred mark, the mobile
 * threads pane's row mark, the desktop thread pane's mark) and an anchored panel
 * put it somewhere different for each; centred, there is one place to look. It
 * is also what lets the mark keep its position in a fixed-width cluster without
 * the menu inheriting that constraint. Centred on the THREAD PANE where there is
 * one (desktop), on the viewport where the pane is the whole screen (mobile):
 * see `.brand-menu` in styles/header-mark.css.
 *
 * Every host keeps its OWN open state and passes its own element as `anchor`.
 * An `anchor={null}` toggle-opened overlay is a known bug shape: on touch the
 * outside-pointerdown dismiss closes it and the toggle's own `touchend`
 * immediately reopens it, so it never closes. See `.claude/rules/frontend.md`
 * § Modals & Popovers.
 */
function LucidosMenu({ open, onClose, anchor, actionsInRow, homeInRow }: {
  open: boolean;
  onClose: () => void;
  anchor: HTMLElement | null;
  /** The host's own header row already carries New thread, Search everywhere
   *  and Setup interview as icons, so the menu does not repeat them.
   *  True on desktop only: no mobile header has room for them, which is why
   *  they came into the menu in the first place. What is left is Workspaces and
   *  Refresh, the pair the retired workspace switcher owned and the only part of
   *  this menu desktop can reach nowhere else. */
  actionsInRow: boolean;
  /** The host's row carries a Home icon, so the menu drops its Home row. True
   *  on both thread pane rows; the thread drawer's row has no Home icon. */
  homeInRow: boolean;
}) {
  return (
    <>
      {/* Centred from CSS, so no measure pass and no hidden first frame: the
          panel is `position: fixed` and portaled to <body> so its offsets
          resolve against the viewport. */}
      {/* `role="menu"`, not `dialog`: the toggle advertises `aria-haspopup="menu"`
          and every row is a `menuitem`, and a `menuitem` must be owned by a
          menu-role container to mean anything. Centred is a PLACEMENT; it
          does not make this a dialog. */}
      <Overlay
        open={open}
        onClose={onClose}
        anchor={anchor}
        backdrop={false}
        portal
        panelClass="surface-box surface-raised brand-menu"
        panelRole="menu"
        panelProps={{ 'aria-label': MENU_NAME }}
      >
        <BrandMenuRows onClose={onClose} actionsInRow={actionsInRow} homeInRow={homeInRow} />
      </Overlay>
    </>
  );
}

/**
 * The Lucidos mark, doing three jobs at once: it is the brand, it is the
 * connection light, and it opens the menu.
 *
 * `placement` names the header row the mark sits on. That decides how it is
 * DRESSED, and what it opens. The menu drops any row whose action the host's
 * row already carries as an icon (`actionsInRow`, `homeInRow`).
 *
 * - `cluster` (the mobile thread pane) is the centrepiece of a fixed-width nav
 *   cluster, at the larger tap target, and it is the connection light:
 *   `data-conn` drives the colour-and-motion ladder in styles/header-mark.css.
 * - `brand` (the desktop thread pane) is the same control on a row with room to
 *   spare, so it keeps the mark's own dressing and the connection light, adds
 *   the hover tooltip its neighbours in that bar all carry, and trims the menu.
 *   It replaced the `[Lucidos * workspace]` wordmark label; the workspace name
 *   beside it is now a plain label (`WorkspaceNameLabel`), not a second opener.
 * - `row` (the mobile threads pane) is DRESSED as a member of that row's icon
 *   run without sitting in it. It wears `.icon-btn.header-icon` so the run's
 *   BOX comes from the same rules its neighbours use, which is the only way to
 *   be sure it reads at their size; its colour departs from theirs, holding the
 *   bar's full foreground rather than the muted glyph colour, so the brand
 *   reads at the same strength on both panes (styles/header-mark.css). Where it
 *   SITS is the centred nav cluster's trailing edge, the column the other two
 *   rows give their forward chevron, so the mark does not move as the user
 *   swipes between panes (see MobileAppHeader). It carries NO `data-conn`: a
 *   glyph that dims itself among a row of icons that do not reads as disabled
 *   rather than as disconnected. One connection light, on the pane that owns
 *   it.
 */
export function BrandMenuButton({ placement = 'cluster' }: { placement?: 'cluster' | 'brand' | 'row' }) {
  const [anchor, setAnchor] = useState<HTMLElement | null>(null);
  const [open, setOpen] = useState(false);
  const pendingOpen = usePendingOpen();
  const status = connectionStatus.value;
  const inRow = placement === 'row';
  const onDesktop = placement === 'brand';
  // The degraded states spend the connection half on the notice rather than on
  // the phrase alone. Naming the state is enough while everything works, and is
  // the least useful half of what we know once it does not. Same table as the
  // bar, the menu notice and the tooltip below, so none of them can disagree.
  const workspace = visibleWorkspaceName.value;
  const sentence = connectionNoticeSentence(status, workspace, 'short');
  // The unread count is spoken HERE rather than on the badge that draws it.
  // That badge is `pointer-events: none` and aria-hidden. Neither a pointer nor
  // a screen reader reaches it; the mark is the element both land on. Without
  // this the count would be silent on the two panes the bell never appears on,
  // which is the whole reason it rides the mark.
  const unreadCount = crossWorkspaceUnreadTotal.value;
  // The state badge is click-through for the same reason, so the mark speaks
  // what it stands for too: the jobs in flight, or a version to take.
  const activity = liveActivityRows();
  const label = [
    MENU_NAME,
    unreadBadgeLabel(unreadCount),
    brandBadgeLabel(activity),
    inRow ? null : (sentence ?? connectionPhrase(status, workspace)),
  ].filter(Boolean).join(' · ');
  // The hover tooltip says the same at a glance, as a label/value grid. The
  // full sentence would wrap to a paragraph of thread titles the menu lists.
  const tooltipRows = onDesktop
    ? brandTooltipRows(unreadCount, activity, connectionNotice(status, workspace, 'short'))
    : [];

  return (
    <>
      {/* The badges are SIBLINGS of the mark, not children of it. The slot is
          the positioning context they share, and it is what puts each badge ON
          the mark's corner rather than in a flex slot beside it
          (styles/header-mark.css). Both badges are click-through there, so a
          tap anywhere on the slot is a tap on the mark, which opens the menu.
          The menu's activity group is what the state badge stands for. */}
      <span class="brand-mark-slot">
        <button
          ref={setAnchor}
          type="button"
          class={inRow ? 'icon-btn header-icon brand-mark-row' : `brand-mark${open ? ' is-open' : ''}`}
          data-role="brand-menu-toggle"
          // Only the cluster mark is the connection light, so only it carries
          // the attribute the state rules key on (see the component doc).
          data-conn={inRow ? undefined : status}
          onClick={(e) => {
            // Re-read the peer counts on the way in, so the notifications group
            // is current rather than as of the last resume or tick.
            //
            // `WorkspacesMenuRow` deliberately hangs its own fetch off its
            // EXPAND instead, and that rule still holds for it: it has nothing
            // cached, so a fetch there would put Refresh and Restart behind a
            // spinner. The group does have a cache, renders from it instantly,
            // and this only corrects it.
            if (open) { setOpen(false); return; }
            if (pendingOpen.takeBack()) return;
            void refreshOtherWorkspacesUnread();
            pendingOpen.open(BrandMenuRows, () => setOpen(true), e.currentTarget);
          }}
          aria-haspopup="menu"
          aria-expanded={open}
          // Colour and motion carry the connection state visually on the cluster
          // mark; this is the half a screen reader gets. The row mark does not
          // show the state, so it does not claim to.
          aria-label={label}
          // A hover tooltip only on the desktop row: there the mark took the
          // place of a label that said "Lucidos" in words, and every other
          // control in that bar names itself on hover. `data-tooltip` is a
          // desktop-only surface anyway, so this is about which ROW earns one,
          // not about which viewport can show it. With nothing to report, the
          // name stands alone.
          data-tooltip={onDesktop && tooltipRows.length === 0 ? MENU_NAME : undefined}
          data-tooltip-title={tooltipRows.length > 0 ? MENU_NAME : undefined}
          data-tooltip-rows={tooltipRows.length > 0 ? JSON.stringify(tooltipRows) : undefined}
        >
          {inRow ? <LucidosMarkIcon /> : <span class="brand-mark-glyph"><LucidosMarkIcon /></span>}
        </button>
        <BrandBadge />
        {/* The unread count, on the OPPOSITE corner from the state badge above
            so the two coexist. See `UnreadBrandBadge` for why it is the bottom
            one, and why it is not a button. */}
        <UnreadBrandBadge />
      </span>

      <LucidosMenu
        open={open}
        onClose={() => setOpen(false)}
        anchor={anchor}
        actionsInRow={onDesktop}
        homeInRow={!inRow}
      />
    </>
  );
}
