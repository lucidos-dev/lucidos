
import { connectionNoticeStatus, visibleWorkspaceName, searchEverywhereOpen, searchEverywhereAnchor, llmConfigured, lucidosRelease, lucidosReleaseDirty, releaseCheck, whatsNewSeenRelease } from '../../store/store';
import { hasUnreadWhatsNew } from '../../store/actions/whatsNew';
import { unfocusThread } from '../../store/actions/threads';
import { homeThreadId, openHomeThread } from '../../store/actions/homeThread';
import { openWhatsNew } from '../../store/actions/menu';
import { connectionNotice, type ConnectionNoticeStatus } from '../../utils/connectionNotice';
import { withUpdateChannel } from '../../utils/updateChannel';
import { lucidosVersionLabel, lucidosVersionTooltip } from '../../utils/lucidosVersion';
import { composeHandlers } from '../chat/composeHandlers';
import { focusSearchInput } from '../search/searchEverywhereActions';
import { ComposeIcon, SearchIcon, HelpIcon, HomeIcon, LucidosMarkIcon } from '../shared/icons';
import { confirmAndStartSetupInterview } from '../shared/setupInterview';
import { NotificationsMenuGroup } from './NotificationsMenuRows';
import { ActivityMenuGroup } from './ActivityMenuRows';
import { WorkspaceRefreshRow, WorkspaceRestartRow } from './WorkspaceMenuRows';
import { WorkspacesMenuRow } from './WorkspaceSwitcher';

export function connectionNoticeRow(status: ConnectionNoticeStatus, workspace: string | null) {
  const notice = connectionNotice(status, workspace, 'short');
  if (!notice) return null;
  return (
    <div class={`brand-menu-notice brand-menu-notice-${status}`} role="none">
      <span class={`status-dot ${status}`} aria-hidden="true" />
      <span class="brand-menu-notice-text">
        <b>{notice.title}</b>{' '}{notice.detail}
      </span>
    </div>
  );
}

export function HomeMenuGroup({ onClose }: { onClose: () => void }) {
  if (!homeThreadId.value) return null;
  return (
    <>
      <button
        type="button"
        class="brand-menu-item home-thread-btn"
        role="menuitem"
        onClick={() => { onClose(); openHomeThread(); }}
      >
        <HomeIcon />
        Home
      </button>
      <div class="brand-menu-separator" role="separator" />
    </>
  );
}

export function BrandMenuRows({ onClose, actionsInRow, homeInRow }: {
  onClose: () => void;
  actionsInRow: boolean;
  homeInRow: boolean;
}) {
  const release = lucidosRelease.value;
  const releaseDirty = lucidosReleaseDirty.value;
  const version = withUpdateChannel(lucidosVersionLabel(release, releaseDirty), release ? releaseCheck.value?.channel : undefined);
  const tooltip = lucidosVersionTooltip(release, releaseDirty);
  const versionTooltip = tooltip ? withUpdateChannel(tooltip, releaseCheck.value?.channel) : undefined;
  const unread = hasUnreadWhatsNew(release, whatsNewSeenRelease.value);
  const versionLabel = unread ? `${versionTooltip} · new release notes` : versionTooltip;
  const newThread = composeHandlers(() => {
    onClose();
    unfocusThread();
  });

  const searchEverywhere = composeHandlers(
    () => {
      onClose();
      searchEverywhereAnchor.value = null;
      searchEverywhereOpen.value = true;
    },
    focusSearchInput,
  );

  const setupInterview = composeHandlers(
    () => {
      onClose();
      void confirmAndStartSetupInterview();
    },
    () => {},
  );
  return (
    <>
      {connectionNoticeRow(connectionNoticeStatus.value, visibleWorkspaceName.value)}

      {!homeInRow && <HomeMenuGroup onClose={onClose} />}

      <ActivityMenuGroup onClose={onClose} />

      <NotificationsMenuGroup onClose={onClose} />

      <button
        type="button"
        class="brand-menu-item brand-menu-version"
        role="menuitem"
        aria-label={versionLabel}
        data-tooltip={versionLabel}
        onClick={() => { onClose(); openWhatsNew(); }}
      >
        <LucidosMarkIcon />
        Lucidos
        <span class="brand-menu-value">
          {unread && <span class="brand-menu-unread-dot" aria-hidden="true" />}
          <span class="brand-menu-value-name">{version}</span>
        </span>
      </button>
      <div class="brand-menu-separator" role="separator" />

      {!actionsInRow && (
        <>
          <button type="button" class="brand-menu-item" role="menuitem" {...newThread}>
            <ComposeIcon />
            New thread
          </button>
          <button type="button" class="brand-menu-item" role="menuitem" {...searchEverywhere}>
            <SearchIcon />
            Search everywhere
          </button>
          {llmConfigured.value && (
            <button type="button" class="brand-menu-item" role="menuitem" {...setupInterview}>
              <HelpIcon />
              Setup guide
            </button>
          )}
        </>
      )}
      <WorkspacesMenuRow onClose={onClose} />
      <WorkspaceRefreshRow onClose={onClose} />
      <WorkspaceRestartRow onClose={onClose} />
    </>
  );
}
