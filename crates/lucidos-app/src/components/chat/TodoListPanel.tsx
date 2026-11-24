import { useRef } from 'preact/hooks';
import { useAnchoredPosition } from '../../hooks/useAnchoredPopover';
import { focusedThreadId, threadMap } from '../../store/store';
import type { TodoItem, TodoStatus } from '../../store/thread-events';
import { Overlay } from '../shared/Overlay';
import { SurfaceHead } from '../shared/Surface';
import { closeTodoPanel, todoPanelAnchor } from './todoIndicator';

const TODO_MARKER: Record<TodoStatus, string> = {
  pending: '○',
  in_progress: '◐',
  completed: '✓',
  waiting: '◷',
  abandoned: '⊘',
};

const TODO_STATUS_TAG: Partial<Record<TodoStatus, string>> = {
  waiting: 'waiting',
  abandoned: 'abandoned',
};

export function todoListPanelBody({
  items,
  notes,
  onClose,
}: {
  items: TodoItem[];
  notes?: string | null;
  onClose: () => void;
}) {
  return (
    <>
      <SurfaceHead title="Todo list" onClose={onClose} closeLabel="Close todo list" />
      <div class="anchored-popover-body">
        {notes ? (
          <div class="todo-panel-notes" data-role="todo-notes">
            <span class="todo-panel-notes-label">Notes</span>
            <p class="todo-panel-notes-body">{notes}</p>
          </div>
        ) : null}
        <ul class="todo-panel-list">
          {items.map((item, idx) => {
            const tag = TODO_STATUS_TAG[item.status];
            return (
              <li
                key={idx}
                class="todo-panel-row"
                data-status={item.status}
              >
                <span class="todo-panel-marker" aria-hidden="true">
                  {TODO_MARKER[item.status] ?? TODO_MARKER.pending}
                </span>
                <span class="todo-panel-text">
                  {item.status === 'in_progress' ? item.active_form : item.content}
                </span>
                {tag ? (
                  <span class="label label-neutral todo-panel-status-tag" aria-label={tag}>
                    {tag}
                  </span>
                ) : null}
              </li>
            );
          })}
        </ul>
      </div>
    </>
  );
}

export function TodoPanelHost() {
  const panelRef = useRef<HTMLDivElement>(null);
  const id = focusedThreadId.value;
  const meta = id ? threadMap.value.get(id)?.meta : undefined;
  const items = meta?.latestTodoList ?? null;
  const notes = meta?.latestTodoNotes ?? null;
  const anchor = todoPanelAnchor.value;
  const isOpen = anchor !== null && !!items && (items.length > 0 || !!notes);
  const pos = useAnchoredPosition(isOpen ? anchor : null, panelRef, '.thread-pane');

  return (
    <Overlay
      open={isOpen}
      onClose={closeTodoPanel}
      anchor={anchor}
      backdrop={false}
      portal
      panelClass="surface anchored-popover todo-panel"
      panelStyle={pos
        ? {
            top: `${pos.top}px`,
            left: `${pos.left}px`,
            '--anchored-popover-fit': `${pos.maxWidth}px`,
          }
        : { visibility: 'hidden' }}
      panelRole="dialog"
      panelProps={{ 'aria-label': 'Current todo list' }}
      dataRole="todo-panel"
      panelRef={panelRef}
    >
      {items
        && (items.length > 0 || notes)
        && todoListPanelBody({ items, notes, onClose: closeTodoPanel })}
    </Overlay>
  );
}
