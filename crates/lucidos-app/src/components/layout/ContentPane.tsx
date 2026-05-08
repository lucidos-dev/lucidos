import { useRef } from 'preact/hooks';
import { activeMenuItem, panelOverlay, parseRepoPath, type PanelOverlay } from '../../store/store';
import { useScrollMemory, contentScrollKey } from '../../hooks/useScrollMemory';
import { FilesView } from '../files/FilesView';
import { AppsView } from '../apps/AppsView';
import { TriggersView } from '../triggers/TriggersView';
import { SettingsView } from '../settings/SettingsView';
import { ChangesView } from '../changes/ChangesView';
import { NotificationsView } from '../notifications/NotificationsView';
import { FilePreviewInline } from '../files/FilePreviewInline';
import { RepoFilePreviewWithSidebar } from '../files/RepoFilePreview';
import { UrlPreviewInline } from '../files/UrlPreviewInline';
import { AppUiInline } from '../apps/AppUiInline';
import { InlineForm } from './InlineForm';

// Per-overlay scoping so scroll resets when switching between, say, two file
// previews. Returns null when there's nothing to key on, so the hook skips.
function contentViewKey(active: string | null, overlay: PanelOverlay): string | null {
  if (overlay) {
    if (overlay.type === 'file-preview') return `file:${overlay.path}`;
    if (overlay.type === 'url-preview') return `url:${overlay.url}`;
    return overlay.type;
  }
  return active;
}

/** Rendered twice — once by SplitLayout, once by MobileSwipeContainer. The
 *  `layout` prop lets dual-render-aware children skip work in the inactive
 *  copy. */
export function ContentPane({ layout }: { layout: 'desktop' | 'mobile' }) {
  const active = activeMenuItem.value;
  const overlay = panelOverlay.value;

  const isAppUi = overlay?.type === 'app-ui';

  const bodyRef = useRef<HTMLDivElement>(null);
  const viewKey = contentViewKey(active, overlay);
  // resetOnEmpty: this body hosts every view; without it, a stale scrollTop
  // from the prior view persists on the DOM and reappears when content grows.
  useScrollMemory(bodyRef, viewKey ? contentScrollKey(viewKey) : null, { resetOnEmpty: true });

  return (
    <div class="content-pane">
      <div class={`content-pane-body${isAppUi ? ' has-app-ui' : ''}`} ref={bodyRef}>

        {overlay?.type === 'form' && <InlineForm />}
        {overlay?.type === 'app-ui' && <AppUiInline layout={layout} />}
        {overlay?.type === 'file-preview' && (() => {
          const repo = parseRepoPath(overlay.path);
          return repo
            ? <RepoFilePreviewWithSidebar repoId={repo.repoId} mode={repo.mode} path={repo.path} changeId={repo.changeId} layout={layout} />
            : <FilePreviewInline path={overlay.path} layout={layout} />;
        })()}
        {overlay?.type === 'url-preview' && <UrlPreviewInline url={overlay.url} layout={layout} />}
        {!overlay && (
          <>
            {active === 'files' && <FilesView />}
            {active === 'apps' && <AppsView />}
            {active === 'triggers' && <TriggersView />}
            {active === 'settings' && <SettingsView />}
            {active === 'changes' && <ChangesView />}
            {active === 'notifications' && <NotificationsView />}
          </>
        )}
      </div>
    </div>
  );
}
