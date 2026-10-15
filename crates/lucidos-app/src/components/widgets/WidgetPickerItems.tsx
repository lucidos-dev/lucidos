import type { OverflowMenuContext } from '../shared/OverflowMenu';
import { AppIcon } from '../shared/AppIcon';
import { focusPromptNow } from '../chat/promptFocus';
import { appendMessagesToCompose } from '../../store/actions/chat';
import { isHomeThread } from '../../store/actions/homeThread';
import { focusThread } from '../../store/actions/threads';
import { pinWidgetToShelf } from '../../store/actions/widget-actions';
import type { App } from '../../store/types';
import { reusableWidgets } from '../../store/widgets';

/** Pin a reusable widget to this thread's shelf, or to Home's board in Home.
 *  One that needs params goes to the thread as a drafted prompt, so the agent
 *  asks for them. */
function addWidget(widget: App, threadId: string): void {
  const required = Object.entries(widget.params ?? {}).filter(([, p]) => p.required).map(([n]) => n);
  if (required.length === 0) {
    void pinWidgetToShelf({ appId: widget.id, threadId }, widget.name);
    return;
  }
  const place = isHomeThread(threadId) ? 'Home' : "this thread's shelf";
  focusThread(threadId);
  appendMessagesToCompose(threadId, [{
    text: `Pin the ${widget.name} widget (\`${widget.id}\`) to ${place}. Ask me for its ${required.join(', ')}.`,
    imageHashes: [],
  }]);
  focusPromptNow();
}

/** The installed reusable widgets, as menu rows that add one to `threadId`.
 *  The Home board's Add and the thread menu's Add widget list the same rows.
 *  A plugin in a catalog has nothing on disk, so it is never listed and runs
 *  no code before install. The caller loads the list as its menu opens. */
export function WidgetPickerItems({ run, threadId }: { run: OverflowMenuContext['run']; threadId: string }) {
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
          onClick={run(() => addWidget(w, threadId))}
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
