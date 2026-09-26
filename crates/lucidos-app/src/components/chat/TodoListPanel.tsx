import { useRef } from 'preact/hooks';
import { useAnchoredPosition } from '../../hooks/useAnchoredPopover';
import { focusedThreadId, threadMap } from '../../store/store';
import type { TodoItem, TodoStatus } from '../../store/thread-events';
import { CloseIcon } from '../shared/icons';
import { Overlay } from '../shared/Overlay';
import { closeTodoPanel, todoPanelAnchor } from './todoIndicator';

/** One marker glyph per status, all from the same geometric-circle family so
 *  the 1rem marker column reads as one column. `waiting`'s clock face echoes
 *  the *waiting indicator*'s own clock icon beside it in the prompt bar:
 *  both say the same thing, that something else will wake this. */
const TODO_MARKER: Record<TodoStatus, string> = {
  pending: '○',
  in_progress: '◐',
  completed: '✓',
  waiting: '◷',
  abandoned: '⊘',
};

/** The two engine-written statuses wear a word, because they are the two the
 *  user did not watch happen and the glyph alone would not explain. The three
 *  the agent writes are self-evident from the row's own styling. */
const TODO_STATUS_TAG: Partial<Record<TodoStatus, string>> = {
  waiting: 'waiting',
  abandoned: 'abandoned',
};

/** `data-status` is stamped on each row so CSS can branch on it.
 *
 *  Returns the panel's CONTENTS, not its box: the box is the `<Overlay>` panel
 *  itself, which is what `useAnchoredPosition` measures and positions. */
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
      <div class="prompt-bar-popover-head">
        <span class="prompt-bar-popover-title">Todo list</span>
        <button
          type="button"
          class="icon-btn prompt-bar-popover-close"
          aria-label="Close todo list"
          onClick={onClose}
        >
          <CloseIcon />
        </button>
      </div>
      <div class="prompt-bar-popover-body">
        {/* The agent's own *todo notes*, above the list because they are what
            it kept rather than what it planned. Rendered only when there are
            any, which under ADR 0085's context mode is most lists and
            otherwise none: the panel is unchanged for a list without them. */}
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
                {/* The fallback is NOT dead code the types make unreachable:
                    `TodoStatus` is a compile-time claim, and the events arriving
                    over SSE come from whatever engine is running now, which after
                    an Apply and restart can be newer than this loaded client. An
                    unrecognized status then renders as an ordinary open item
                    rather than a blank marker column. */}
                <span class="todo-panel-marker" aria-hidden="true">
                  {TODO_MARKER[item.status] ?? TODO_MARKER.pending}
                </span>
                {/* Only an in-progress item is being worked, so only it renders
                    the present-continuous form. A parked one is not: "Running
                    tests" on a thread asleep on an event wait would claim
                    activity that stopped. */}
                <span class="todo-panel-text">
                  {item.status === 'in_progress' ? item.active_form : item.content}
                </span>
                {tag ? (
                  <span class="todo-panel-status-tag" aria-label={tag}>
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

/** The panel itself, mounted by the composer rather than by the control.
 *
 *  It has to outlive the fold: the control it belongs to moves between the row
 *  and the ⋯ menu, and neither is a place a panel can live. Portaled anyway, so
 *  where it sits in the tree decides nothing.
 *
 *  The Overlay panel IS the `.todo-panel` box, placed by `useAnchoredPosition`
 *  rather than by CSS, and portaled because the composer's ancestors animate
 *  `transform`. Same wiring as the waiting panel beside it; the dismiss
 *  contract lives in <Overlay>. */
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
      panelClass="prompt-bar-popover todo-panel"
      // `--prompt-bar-popover-fit` is the thread pane's usable width, the box
      // the hook clamped this panel's position into (see WaitingPanel).
      panelStyle={pos
        ? {
            top: `${pos.top}px`,
            left: `${pos.left}px`,
            '--prompt-bar-popover-fit': `${pos.maxWidth}px`,
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
