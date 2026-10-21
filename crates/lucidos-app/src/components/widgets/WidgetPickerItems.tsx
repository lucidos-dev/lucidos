import type { OverflowMenuContext } from '../shared/OverflowMenu';
import { AppIcon } from '../shared/AppIcon';
import { focusPromptNow } from '../chat/promptFocus';
import { appendMessagesToCompose } from '../../store/actions/chat';
import { addToDashboard } from '../../store/actions/dashboards';
import { homeThreadId } from '../../store/actions/homeThread';
import { focusThread } from '../../store/actions/threads';
import { pinWidgetToShelf } from '../../store/actions/widget-actions';
import { dashboardFor } from '../../store/dashboards';
import { appsList, showToast } from '../../store/store';
import type { App } from '../../store/types';
import { reusableWidgets } from '../../store/widgets';
import type { Dashboard } from '../../generated/thread-event-wire';

/** Where a pick goes: a thread's shelf, or a dashboard. */
export type WidgetPickerTarget = { threadId: string } | { dashboard: Dashboard };

/** Where a drafted prompt asks for the params, and what it asks the agent to
 *  do. A dashboard has no chat of its own, so its prompt goes to Home. */
function paramsPrompt(widget: App, target: WidgetPickerTarget): { threadId: string | null; ask: string } {
  const named = `the ${widget.name} widget (\`${widget.id}\`)`;
  if ('threadId' in target) return { threadId: target.threadId, ask: `Pin ${named} to this thread's shelf.` };
  return { threadId: homeThreadId.value, ask: `Add ${named} to the ${target.dashboard} dashboard.` };
}

/** Add a reusable widget where `target` names. One that needs params goes to
 *  a thread as a drafted prompt, so the agent asks for them. */
function addWidget(widget: App, target: WidgetPickerTarget): void {
  const required = Object.entries(widget.params ?? {}).filter(([, p]) => p.required).map(([n]) => n);
  if (required.length === 0) {
    if ('threadId' in target) void pinWidgetToShelf({ appId: widget.id, threadId: target.threadId }, widget.name);
    else void addToDashboard(target.dashboard, { appId: widget.id }, widget.name, 'widget');
    return;
  }
  const { threadId, ask } = paramsPrompt(widget, target);
  if (!threadId) {
    showToast(`Couldn't add widget "${widget.name}": it needs ${required.join(', ')}, and there is no Home thread to ask in`, 'error');
    return;
  }
  focusThread(threadId);
  appendMessagesToCompose(threadId, [{ text: `${ask} Ask me for its ${required.join(', ')}.`, imageHashes: [] }]);
  focusPromptNow();
}

/** The installed reusable widgets, as menu rows that add one to `target`.
 *  A dashboard's Add and the thread menu's Add widget list the same rows. A
 *  dashboard's Add lists the installed apps below them, each an app shortcut.
 *  A plugin in a catalog has nothing on disk, so it is never listed and runs
 *  no code before install. The caller loads the list as its menu opens. */
export function WidgetPickerItems({ run, target }: { run: OverflowMenuContext['run']; target: WidgetPickerTarget }) {
  return (
    <>
      <WidgetRows run={run} target={target} />
      {'dashboard' in target && <AppShortcutRows run={run} dashboard={target.dashboard} />}
    </>
  );
}

function WidgetRows({ run, target }: { run: OverflowMenuContext['run']; target: WidgetPickerTarget }) {
  const widgets = reusableWidgets.value;
  if (widgets.status === 'failed') {
    return <div class="thread-overflow-note thread-overflow-note-error" role="alert">Couldn't list widgets: {widgets.error}</div>;
  }
  if (widgets.status !== 'loaded') {
    return <div class="thread-overflow-note" aria-hidden="true" style={{ visibility: 'hidden' }}>No widgets to add.</div>;
  }
  if (widgets.data.length === 0) {
    return <div class="thread-overflow-note">No widgets to add. Ask Lucidos to make one.</div>;
  }
  return (
    <>
      {widgets.data.map((w) => (
        <PickerRow key={w.id} app={w} attr={{ 'data-add-widget': w.id }} onPick={run(() => addWidget(w, target))} />
      ))}
    </>
  );
}

/** The apps this dashboard does not yet show, as rows that add a shortcut. */
function AppShortcutRows({ run, dashboard }: { run: OverflowMenuContext['run']; dashboard: Dashboard }) {
  const apps = appsList.value;
  const view = dashboardFor(dashboard);
  if (apps.status === 'failed') {
    return <div class="thread-overflow-note thread-overflow-note-error" role="alert">Couldn't list apps: {apps.error}</div>;
  }
  if (apps.status !== 'loaded') return null;
  const held = new Set(view.status === 'loaded' ? view.data.tiles.filter((t) => t.kind === 'app').map((t) => t.app_id) : []);
  const addable = apps.data.filter((a) => !held.has(a.id));
  if (addable.length === 0) return null;
  return (
    <>
      <div class="thread-overflow-divider" role="separator" />
      <div class="thread-overflow-note">Apps</div>
      {addable.map((a) => (
        <PickerRow
          key={a.id}
          app={a}
          attr={{ 'data-add-app': a.id }}
          onPick={run(() => void addToDashboard(dashboard, { appId: a.id }, a.name, 'app'))}
        />
      ))}
    </>
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
