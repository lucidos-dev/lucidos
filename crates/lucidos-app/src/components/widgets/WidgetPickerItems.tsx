import type { OverflowMenuContext } from '../shared/OverflowMenu';
import { AppIcon } from '../shared/AppIcon';
import { focusPromptNow } from '../chat/promptFocus';
import { appendMessagesToCompose } from '../../store/actions/chat';
import { addToDashboard } from '../../store/actions/dashboards';
import { homeThreadId } from '../../store/actions/homeThread';
import { focusThread } from '../../store/actions/threads';
import { pinWidgetToShelf } from '../../store/actions/widget-actions';
import { showToast } from '../../store/store';
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
    else void addToDashboard(target.dashboard, { appId: widget.id }, widget.name);
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
 *  A dashboard's Add and the thread menu's Add widget list the same rows.
 *  A plugin in a catalog has nothing on disk, so it is never listed and runs
 *  no code before install. The caller loads the list as its menu opens. */
export function WidgetPickerItems({ run, target }: { run: OverflowMenuContext['run']; target: WidgetPickerTarget }) {
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
        <button
          key={w.id}
          type="button"
          role="menuitem"
          class="thread-overflow-item widget-menu-item"
          data-add-widget={w.id}
          onClick={run(() => addWidget(w, target))}
        >
          <AppIcon appId={w.id} name={w.name} icon={w.icon} />
          <span class="widget-menu-label">
            <span>{w.name}</span>
            {w.description && <span class="widget-menu-hint">{w.description}</span>}
          </span>
        </button>
      ))}
    </>
  );
}
