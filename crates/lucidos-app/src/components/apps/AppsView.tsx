import { appsList } from '../../store/store';
import {
  openApp,
  createNewApp,
  confirmDeleteApp,
  openEditApp,
} from '../../store/actions/apps';
import { useDelayedLoading } from '../../hooks/useDelayedLoading';
import { AppRow } from './AppCard';

export function AppsView() {
  const loadable = appsList.value;
  const showLoading = useDelayedLoading(loadable);
  if (loadable.status === 'failed') {
    return (
      <div class="content-view active">
        <div class="list-rows">
          <div class="empty-state error-text">Failed to load apps: {loadable.error}</div>
        </div>
      </div>
    );
  }

  if (loadable.status !== 'loaded') {
    if (!showLoading) return null;
    return (
      <div class="content-view active">
        <div class="list-rows">
          <div class="loading-spinner" />
        </div>
      </div>
    );
  }

  return (
    <div class="content-view active">
      <div class="list-rows">
        {loadable.data.map((app) => (
          <AppRow
            key={app.id}
            app={app}
            onOpen={() => openApp(app)}
            onEdit={() => openEditApp(app.id)}
            onDelete={() => confirmDeleteApp(app.id, app.name)}
          />
        ))}
        <div class="list-row-add-card" onClick={createNewApp}>
          <div class="list-row-add-icon">+</div>
          <div class="list-row-add-label">New App</div>
        </div>
      </div>
    </div>
  );
}
