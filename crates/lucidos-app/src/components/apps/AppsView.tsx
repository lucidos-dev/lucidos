import { appsList, appSearchQuery } from '../../store/store';
import {
  openApp,
  createNewApp,
  confirmDeleteApp,
  openEditApp,
  refreshApps,
} from '../../store/actions/apps';
import { usePanelRefresh } from '../../hooks/usePanelRefresh';
import { useDelayedLoading } from '../../hooks/useDelayedLoading';
import { LoadableError } from '../shared/LoadableError';
import { ListRowAddCard } from '../shared/ListRowAddCard';
import { ListSkeletonOf } from '../shared/Skeleton';
import { LoadingFade } from '../shared/LoadingFade';
import { AppRow } from './AppCard';
import { AppSearchBar } from './AppSearchBar';
import { ReusableWidgetsSection } from './ReusableWidgetsSection';
import { loadReusableWidgets } from '../../store/actions/widgets';

/** The apps list and the reusable widgets under it. */
function refreshAppsPanel(): Promise<unknown> {
  return Promise.all([
    refreshApps(),
    loadReusableWidgets(),
  ]);
}

export function AppsView() {
  usePanelRefresh('apps', refreshAppsPanel);
  const loadable = appsList.value;
  const showLoading = useDelayedLoading(loadable);

  let body;
  if (loadable.status === 'failed') {
    body = <LoadableError noun="apps" error={loadable.error} />;
  } else {
    body = (
      <LoadingFade showSkeleton={showLoading} skeleton={<ListSkeletonOf fill containerClass="list-rows" row={() => <AppRow />} />}>
        {loadable.status === 'loaded'
          ? (() => {
              const query = appSearchQuery.value.trim().toLowerCase();
              const apps = query
                ? loadable.data.filter((app) =>
                    app.name.toLowerCase().includes(query) ||
                    (app.description ?? '').toLowerCase().includes(query))
                : loadable.data;

              if (query && apps.length === 0) {
                return <div class="empty-state"><p>No installed apps match "{appSearchQuery.value.trim()}".</p></div>;
              }
              return (
                <div class="list-rows">
                  {apps.map((app) => (
                    <AppRow
                      key={app.id}
                      app={app}
                      onOpen={() => openApp(app)}
                      onEdit={() => openEditApp(app.id)}
                      onDelete={() => void confirmDeleteApp(app.id, app.name)}
                    />
                  ))}
                  {!query && <ListRowAddCard label="New App" onClick={createNewApp} />}
                </div>
              );
            })()
          : null}
      </LoadingFade>
    );
  }

  return (
    <div class="content-view active apps-view">
      <AppSearchBar placeholder="Search apps…" dataRole="apps-search-input" />
      {body}
      <ReusableWidgetsSection />
    </div>
  );
}
