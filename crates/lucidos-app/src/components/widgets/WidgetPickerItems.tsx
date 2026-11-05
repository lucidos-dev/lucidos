import { useRef } from 'preact/hooks';
import { useFocusWhenPlaced } from '../../hooks/useFocusWhenPlaced';
import { isTouchLayout } from '../shared/keyboardHandoff';
import { ControlFilter } from '../shared/ControlOptionList';
import { nextMenuIndex } from '../shared/OverflowMenu';
import { filterListItems, useListFilter } from '../shared/useListFilter';
import type { OverflowMenuContext } from '../shared/OverflowMenu';
import { AppIcon } from '../shared/AppIcon';
import { focusPromptNow } from '../chat/promptFocus';
import { appendMessagesToCompose } from '../../store/actions/chat';
import { addToDashboard } from '../../store/actions/dashboards';
import { homeThreadId } from '../../store/actions/homeThread';
import { focusThread } from '../../store/actions/threads';
import { dashboardFor } from '../../store/dashboards';
import { appsList, showToast } from '../../store/store';
import type { App } from '../../store/types';
import { reusableWidgets } from '../../store/widgets';
import type { Dashboard } from '../../generated/thread-event-wire';

/** Add a reusable widget to `dashboard`. One that needs params goes to Home
 *  as a drafted prompt, so the agent asks for them: a dashboard has no chat
 *  of its own. */
function addWidget(widget: App, dashboard: Dashboard): void {
  const required = Object.entries(widget.params ?? {}).filter(([, p]) => p.required).map(([n]) => n);
  if (required.length === 0) {
    void addToDashboard(dashboard, { appId: widget.id }, widget.name, 'widget');
    return;
  }
  const threadId = homeThreadId.value;
  const ask = `Add the ${widget.name} widget (\`${widget.id}\`) to the ${dashboard} dashboard.`;
  if (!threadId) {
    showToast(`Couldn't add widget "${widget.name}": it needs ${required.join(', ')}, and there is no Home thread to ask in`, 'error');
    return;
  }
  focusThread(threadId);
  appendMessagesToCompose(threadId, [{ text: `${ask} Ask me for its ${required.join(', ')}.`, imageHashes: [] }]);
  focusPromptNow();
}

/** The installed reusable widgets, as menu rows that add one to `dashboard`,
 *  and the installed apps below them, each an app shortcut.
 *  A plugin in a catalog has nothing on disk, so it is never listed and runs
 *  no code before install. The caller loads the list as its menu opens. */
export function WidgetPickerItems({ run, dashboard }: { run: OverflowMenuContext['run']; dashboard: Dashboard }) {
  const listRef = useRef<HTMLDivElement>(null);
  const filter = useListFilter({ enabled: true });
  const widgets = reusableWidgets.value;
  const apps = appsList.value;
  const view = dashboardFor(dashboard);
  const held = new Set(view.status === 'loaded' ? view.data.tiles.filter((t) => t.kind === 'app').map((t) => t.app_id) : []);
  const widgetRows = widgets.status === 'loaded' ? filterListItems(widgets.data, filter.query.value) : [];
  const appRows = apps.status === 'loaded'
    ? filterListItems(apps.data.filter((a) => !held.has(a.id)), filter.query.value) : [];
  const settled = widgets.status === 'loaded' && apps.status === 'loaded';

  useFocusWhenPlaced(() => {
    if (filter.searching) return filter.inputRef.current;
    return isTouchLayout() ? null : listRef.current?.querySelector<HTMLElement>('[role="menuitem"]') ?? listRef.current;
  }, [filter.query.value]);

  function handleKeyDown(e: KeyboardEvent) {
    if (filter.handleKeyDown(e)) return;
    const items = Array.from(listRef.current?.querySelectorAll<HTMLButtonElement>('[role="menuitem"]') ?? []);
    const current = items.indexOf(document.activeElement as HTMLButtonElement);
    if (e.key === 'Enter') {
      e.preventDefault();
      e.stopPropagation();
      items[current < 0 ? 0 : current]?.click();
    } else if (e.key === 'ArrowDown' || e.key === 'ArrowUp') {
      e.preventDefault();
      e.stopPropagation();
      const next = items[nextMenuIndex(current, items.length, e.key)];
      next?.focus({ preventScroll: true });
      next?.scrollIntoView({ block: 'nearest' });
    }
  }

  return (
    <div ref={listRef} tabIndex={0} onKeyDown={handleKeyDown}>
      {filter.visible && <ControlFilter
        value={filter.query.value}
        placeholder="Filter widgets and apps…"
        inputRef={filter.inputRef}
        onInput={filter.setQuery}
      />}
      {widgets.status === 'failed' && (
        <div class="thread-overflow-note thread-overflow-note-error" role="alert">Couldn't list widgets: {widgets.error}</div>
      )}
      {widgets.status !== 'loaded' && widgets.status !== 'failed' && (
        <div class="thread-overflow-note" aria-hidden="true" style={{ visibility: 'hidden' }}>No widgets to add.</div>
      )}
      {widgets.status === 'loaded' && widgets.data.length === 0 && !filter.searching && (
        <div class="thread-overflow-note">No widgets to add. Ask Lucidos to make one.</div>
      )}
      {widgetRows.map((w) => (
        <PickerRow key={w.id} app={w} attr={{ 'data-add-widget': w.id }} onPick={run(() => addWidget(w, dashboard))} />
      ))}
      {apps.status === 'failed' && (
        <div class="thread-overflow-note thread-overflow-note-error" role="alert">Couldn't list apps: {apps.error}</div>
      )}
      {appRows.length > 0 && <>
        {widgetRows.length > 0 && <div class="thread-overflow-divider" role="separator" />}
        <div class="thread-overflow-note">Apps</div>
        {appRows.map((app) => (
          <PickerRow key={app.id} app={app} attr={{ 'data-add-app': app.id }}
            onPick={run(() => void addToDashboard(dashboard, { appId: app.id }, app.name, 'app'))} />
        ))}
      </>}
      {settled && filter.searching && widgetRows.length === 0 && appRows.length === 0 && (
        <div class="thread-overflow-note">No matches</div>
      )}
    </div>
  );
}

function PickerRow({ app, attr, onPick }: { app: App; attr: Record<string, string>; onPick: (e: MouseEvent) => void }) {
  return (
    <button
      type="button"
      role="menuitem"
      class="thread-overflow-item widget-menu-item"
      data-tooltip={app.description || undefined}
      {...attr}
      onClick={onPick}
    >
      <AppIcon appId={app.id} name={app.name} icon={app.icon} />
      <span class="widget-menu-label">
        <span>{app.name}</span>
      </span>
    </button>
  );
}
