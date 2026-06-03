import { useSignal } from '@preact/signals';
import { useRef, useState } from 'preact/hooks';
import type { Ref } from 'preact';
import { focusedThreadId, threadMap } from '../../store/store';
import type { TodoItem } from '../../store/thread-events';
import { TodoCheckIcon, TodoInProgressIcon } from '../shared/icons';
import { useDismissOnOutside } from '../../hooks/useAnchoredPopover';

export function todoListIndicatorBody({
  items,
  onClick,
  buttonRef,
}: {
  items: TodoItem[] | null;
  onClick: () => void;
  buttonRef?: Ref<HTMLButtonElement>;
}) {
  if (items === null || items.length === 0) return null;
  const total = items.length;
  const completed = items.filter((i) => i.status === 'completed').length;
  const abandoned = items.filter((i) => i.status === 'abandoned').length;
  const inProgress = items.find((i) => i.status === 'in_progress');
  // Three honest indicator states:
  //   - in-progress: the agent is actively working an item (shows spinner icon)
  //   - abandoned: no in-progress item AND at least one item was abandoned
  //   - idle: every non-completed item is gone (all done, or nothing pending)
  const state = inProgress
    ? 'in-progress'
    : abandoned > 0
      ? 'abandoned'
      : 'idle';
  const tooltip = inProgress
    ? inProgress.active_form
    : abandoned > 0
      ? `${completed} of ${total} done, ${abandoned} abandoned`
      : `${completed} of ${total} done`;
  const ariaLabel = abandoned > 0
    ? `Todo list: ${completed} of ${total} done, ${abandoned} abandoned. Click to expand.`
    : `Todo list: ${completed} of ${total} done. Click to expand.`;
  return (
    <button
      type="button"
      class="icon-btn header-icon"
      data-role="todo-indicator"
      data-state={state}
      data-tooltip={tooltip}
      aria-label={ariaLabel}
      onClick={onClick}
      data-row-item
      ref={buttonRef}
    >
      {inProgress ? <TodoInProgressIcon /> : <TodoCheckIcon />}
    </button>
  );
}

/** `data-status` is stamped on each row so CSS can branch on it. */
export function todoListPanelBody({
  items,
  onClose,
  panelRef,
}: {
  items: TodoItem[];
  onClose: () => void;
  panelRef?: Ref<HTMLDivElement>;
}) {
  return (
    <div
      class="todo-panel"
      data-role="todo-panel"
      role="dialog"
      aria-label="Current todo list"
      ref={panelRef}
    >
      <ul class="todo-panel-list">
        {items.map((item, idx) => (
          <li
            key={idx}
            class="todo-panel-row"
            data-status={item.status}
          >
            <span class="todo-panel-marker" aria-hidden="true">
              {item.status === 'completed'
                ? '✓'
                : item.status === 'in_progress'
                  ? '◐'
                  : item.status === 'abandoned'
                    ? '⊘'
                    : '○'}
            </span>
            <span class="todo-panel-text">
              {item.status === 'in_progress' ? item.active_form : item.content}
            </span>
            {item.status === 'abandoned' ? (
              <span class="todo-panel-abandoned-tag" aria-label="abandoned">
                abandoned
              </span>
            ) : null}
          </li>
        ))}
      </ul>
      <button
        type="button"
        class="todo-panel-close"
        aria-label="Close todo list"
        onClick={onClose}
      >
        ✕
      </button>
    </div>
  );
}

/** Symmetric to `CCControlMenu`: mounted in the prompt-bar actions row,
 *  hidden when the chat agent hasn't written a list. Reads from
 *  `meta.latestTodoList` (projected in `handleEvent`) so the render path
 *  is O(1) — no walk of the events Map per threadMap flush. */
export function TodoListIndicator() {
  const open = useSignal(false);
  const panelRef = useRef<HTMLDivElement>(null);
  // useState (not useRef) so the dismiss hook re-runs once the button mounts
  // and we have a real anchor to exclude from the outside-click test.
  const [anchorEl, setAnchorEl] = useState<HTMLButtonElement | null>(null);

  const id = focusedThreadId.value;
  const items = id ? threadMap.value.get(id)?.meta.latestTodoList ?? null : null;

  useDismissOnOutside(open.value, panelRef, anchorEl, () => (open.value = false));

  return (
    <>
      {todoListIndicatorBody({
        items,
        onClick: () => (open.value = !open.value),
        buttonRef: setAnchorEl,
      })}
      {open.value && items && items.length > 0 ? (
        <div class="todo-panel-anchor">
          {todoListPanelBody({ items, onClose: () => (open.value = false), panelRef })}
        </div>
      ) : null}
    </>
  );
}
