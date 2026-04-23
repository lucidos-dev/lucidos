import { activeMenuItem, panelOverlay, parseRepoPath } from '../../store/store';
import { FilesView } from '../files/FilesView';
import { AppsView } from '../apps/AppsView';
import { TriggersView } from '../triggers/TriggersView';
import { SettingsView } from '../settings/SettingsView';
import { ChangesView } from '../changes/ChangesView';
import { NotificationsView } from '../notifications/NotificationsView';
import { FilePreviewInline } from '../files/FilePreviewInline';
import { RepoFilePreview } from '../files/RepoFilePreview';
import { UrlPreviewInline } from '../files/UrlPreviewInline';
import { AppUiInline } from '../apps/AppUiInline';
import { InlineForm } from './InlineForm';

export function ContentPane() {
  const active = activeMenuItem.value;
  const overlay = panelOverlay.value;

  const isAppUi = overlay?.type === 'app-ui';

  return (
    <div class="content-pane">
      <div class={`content-pane-body${isAppUi ? ' has-app-ui' : ''}`}>

        {overlay?.type === 'form' && <InlineForm />}
        {overlay?.type === 'app-ui' && <AppUiInline />}
        {overlay?.type === 'file-preview' && (() => {
          const repo = parseRepoPath(overlay.path);
          return repo
            ? <RepoFilePreview repoId={repo.repoId} mode={repo.mode} path={repo.path} />
            : <FilePreviewInline path={overlay.path} />;
        })()}
        {overlay?.type === 'url-preview' && <UrlPreviewInline url={overlay.url} />}
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
