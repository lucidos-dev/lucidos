import { useEffect, useMemo, useState } from 'preact/hooks';
import { connectionNoticeStatus, visibleWorkspaceName } from '../../store/store';
import { notificationsMenuUnreadTotal, refreshOtherWorkspacesUnread } from '../../store/actions/app-badge';
import { connectionNotice, connectionNoticeSentence, connectionPhrase } from '../../utils/connectionNotice';
import { createMenuTapGuard } from '../../utils/menuTapGuard';
import { Overlay } from '../shared/Overlay';
import { LucidosMarkIcon } from '../shared/icons';
import { BrandBadge, UnreadBrandBadge, brandBadgeLabel, brandTooltipRows, unreadBadgeLabel } from './BrandBadge';
import { buildTimerIsLive } from '../../store/actions/backgroundActivity';
import { liveActivityRows } from '../../store/actions/activityRows';
import { lazyComponent } from '../../utils/lazyComponent';
import { usePendingOpen } from '../../hooks/usePendingOpen';
import { prefetchWhenIdle } from '../../utils/idlePrefetch';

const BrandMenuRows = lazyComponent(() => import('./BrandMenuRows').then((m) => m.BrandMenuRows));
prefetchWhenIdle(BrandMenuRows);

export const MENU_NAME = 'Lucidos menu';

function LucidosMenu({ open, onClose, anchor, actionsInRow, homeInRow }: {
  open: boolean;
  onClose: () => void;
  anchor: HTMLElement | null;
  actionsInRow: boolean;
  homeInRow: boolean;
}) {
  const tapGuard = useMemo(createMenuTapGuard, []);
  return (
    <>
      <Overlay
        open={open}
        onClose={onClose}
        anchor={anchor}
        backdrop={false}
        portal
        panelClass="surface-box surface-raised brand-menu"
        panelRole="menu"
        panelProps={{ 'aria-label': MENU_NAME, ...tapGuard }}
      >
        <BrandMenuRows onClose={onClose} actionsInRow={actionsInRow} homeInRow={homeInRow} />
      </Overlay>
    </>
  );
}

export function BrandMenuButton({ placement = 'cluster' }: { placement?: 'cluster' | 'brand' | 'row' }) {
  const [anchor, setAnchor] = useState<HTMLElement | null>(null);
  const [open, setOpen] = useState(false);
  const pendingOpen = usePendingOpen();
  const status = connectionNoticeStatus.value;
  const inRow = placement === 'row';
  const onDesktop = placement === 'brand';
  const workspace = visibleWorkspaceName.value;
  const sentence = connectionNoticeSentence(status, workspace, 'short');
  const unreadCount = notificationsMenuUnreadTotal.value;
  const [now, setNow] = useState(() => Date.now());
  const ticking = onDesktop && buildTimerIsLive();
  useEffect(() => {
    if (!ticking) return;
    const timer = setInterval(() => setNow(Date.now()), 1000);
    return () => clearInterval(timer);
  }, [ticking]);
  const activity = liveActivityRows(ticking ? now : Date.now());
  const label = [
    MENU_NAME,
    unreadBadgeLabel(unreadCount),
    brandBadgeLabel(activity),
    inRow ? null : (sentence ?? connectionPhrase(status, workspace)),
  ].filter(Boolean).join(' · ');
  const tooltipRows = onDesktop
    ? brandTooltipRows(unreadCount, activity, connectionNotice(status, workspace, 'short'))
    : [];

  return (
    <>
      <span class="brand-mark-slot">
        <button
          ref={setAnchor}
          type="button"
          class={inRow ? 'icon-btn header-icon brand-mark-row' : `brand-mark${open ? ' is-open' : ''}`}
          data-role="brand-menu-toggle"
          data-conn={inRow ? undefined : status}
          onClick={(e) => {
            if (open) { setOpen(false); return; }
            if (pendingOpen.takeBack()) return;
            void refreshOtherWorkspacesUnread();
            pendingOpen.open(BrandMenuRows, () => setOpen(true), e.currentTarget);
          }}
          aria-haspopup="menu"
          aria-expanded={open}
          aria-label={label}
          data-tooltip={onDesktop && tooltipRows.length === 0 ? MENU_NAME : undefined}
          data-tooltip-title={tooltipRows.length > 0 ? MENU_NAME : undefined}
          data-tooltip-rows={tooltipRows.length > 0 ? JSON.stringify(tooltipRows) : undefined}
        >
          {inRow ? <LucidosMarkIcon /> : <span class="brand-mark-glyph"><LucidosMarkIcon /></span>}
        </button>
        <BrandBadge />
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
