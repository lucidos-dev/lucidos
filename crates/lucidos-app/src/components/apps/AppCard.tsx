import type { App } from '../../store/types';
import { isAppPinned, togglePinApp } from '../../store/actions/pinnedApps';
import { PinIcon } from '../shared/icons';

interface AppRowProps {
  app: App;
  onOpen: () => void;
  onEdit: () => void;
  onDelete: () => void;
}

export function AppRow({ app, onOpen, onEdit, onDelete }: AppRowProps) {
  const pinned = isAppPinned(app.id);
  return (
    <div class="list-row app-row clickable" onClick={onOpen}>
      <div class="list-row-info">
        <div class="title list-row-name">
          {app.name}
        </div>
        {app.description && (
          <div class="list-row-details">{app.description}</div>
        )}
      </div>
      <div class="list-row-actions">
        <button
          class={`icon-btn ${pinned ? 'pinned' : ''}`}
          onClick={(e) => { e.stopPropagation(); togglePinApp(app.id); }}
          aria-label={pinned ? 'Unpin from menu' : 'Pin to menu'}
        >
          <PinIcon filled={pinned} />
        </button>
        <button class="action-btn" onClick={(e) => { e.stopPropagation(); onEdit(); }}>Edit</button>
        <button class="action-btn action-btn-danger" onClick={(e) => { e.stopPropagation(); onDelete(); }}>Delete</button>
      </div>
    </div>
  );
}
