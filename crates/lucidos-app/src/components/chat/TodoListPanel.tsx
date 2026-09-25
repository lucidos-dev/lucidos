import { signal } from '@preact/signals';
import { useRef } from 'preact/hooks';
import { useAnchoredPosition } from '../../hooks/useAnchoredPopover';
import { focusedThreadId, threadMap } from '../../store/store';
import type { TodoItem, TodoStatus } from '../../store/thread-events';
import type { HeaderActionSpec } from '../layout/headerActions';
import { CloseIcon, TodoListIcon } from '../shared/icons';
import { Overlay } from '../shared/Overlay';

/** What the panel is positioned against, or null while it is closed.
 *
 *  Module-level, and one signal for both ways in. The indicator folds into the
 *  composer row's ⋯ menu on a short row, so the button that opens the panel may
 *  not exist. A press on the row button passes itself. A press on the menu row
 *  passes the ⋯ trigger, the box the reader pressed. */
const todoPanelAnchor = signal<HTMLElement | null>(null);

/** Close the panel. Called by the composer when a fold moves controls around,
 *  since an anchor can leave the DOM under an open panel. */
export function closeTodoPanel(): void {
  todoPanelAnchor.value = null;
}

/** The two values `data-state` can take. A union rather than `string`, so a
 *  third one cannot be stamped without a stylesheet rule to paint it. */
type TodoIndicatorState = 'in-progress' | 'idle';

/** Everything the indicator says about a list, in one place.
 *
 *  `null` when there is nothing to report, which is what hides the control.
 *  *Todo notes* can outlive the items: under ADR 0085's context mode an agent
 *  that finished its plan still writes `todos: []` with a pointer worth
 *  keeping. Hiding the indicator there would make the block the whole mode
 *  rests on the one thing the user cannot see.
 *
 *  TWO states, stamped as `data-state` and painted in todo-list.css over one
 *  shared glyph: `in-progress` takes the accent, `idle` takes the row's
 *  ordinary gray. That is the composer row's whole language, the one the follow
 *  toggle and the *waiting indicator* beside it already speak: accent means
 *  something is live right now.
 *
 *  A parked item and an abandoned one used to paint too, a gray pulse and a
 *  dimmed glyph. Both are gone. An item is `waiting` only while the thread
 *  holds a live event wait, so the pulse said what the waiting indicator
 *  already says in accent. The dim read as a disabled button, for a fact that
 *  is history rather than activity. Both still reach the reader in words,
 *  below and in the panel.
 *
 *  The WORDS keep the fuller picture, and there waiting outranks abandoned
 *  because it is the live fact: a list carrying both has parked items that are
 *  still going somewhere. */
export function todoIndicatorSummary(
  items: TodoItem[] | null,
  notes?: string | null,
): {
  state: TodoIndicatorState;
  tooltip: string;
  ariaLabel: string;
  menuLabel: string;
} | null {
  if (items === null) return null;
  if (items.length === 0) {
    if (!notes) return null;
    return {
      state: 'idle',
      tooltip: 'Notes kept',
      ariaLabel: 'Todo list: no items, notes kept. Click to expand.',
      menuLabel: 'Todo list: notes kept',
    };
  }
  const total = items.length;
  const completed = items.filter((i) => i.status === 'completed').length;
  const waiting = items.filter((i) => i.status === 'waiting').length;
  const abandoned = items.filter((i) => i.status === 'abandoned').length;
  const inProgress = items.find((i) => i.status === 'in_progress');
  const state = inProgress ? 'in-progress' : 'idle';
  const counted = waiting > 0
    ? `${completed} of ${total} done, ${waiting} waiting`
    : abandoned > 0
      ? `${completed} of ${total} done, ${abandoned} abandoned`
      : `${completed} of ${total} done`;
  const tooltip = inProgress ? inProgress.active_form : counted;
  // The aria-label names the state, it does not just count. Colour is now the
  // ONLY visual channel carrying it, and a screen reader cannot read colour.
  // Forced-colors mode overwrites it outright: there --accent collapses to the
  // system foreground, so in-progress and idle paint identically. The tooltip
  // cannot stand in either, being desktop-hover only.
  const said = inProgress ? `${inProgress.active_form}. ${completed} of ${total} done` : counted;
  return {
    state,
    tooltip,
    ariaLabel: `Todo list: ${said}. Click to expand.`,
    // The menu row's words. A folded indicator has no paint at all, so this is
    // the whole of what it reports.
    menuLabel: `Todo list: ${said}`,
  };
}

export function todoListIndicatorBody({
  items,
  notes,
  onClick,
  attrs,
}: {
  items: TodoItem[] | null;
  notes?: string | null;
  onClick: (e: MouseEvent) => void;
  /** The row attributes the composer's fold cluster stamps on every member. */
  attrs?: Record<string, string>;
}) {
  const summary = todoIndicatorSummary(items, notes);
  if (!summary) return null;
  return (
    <button
      {...attrs}
      type="button"
      class="icon-btn header-icon"
      data-role="todo-indicator"
      data-state={summary.state}
      data-tooltip={summary.tooltip}
      aria-label={summary.ariaLabel}
      onClick={onClick}
      data-row-item
    >
      <TodoListIcon />
    </button>
  );
}

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

/** The todo indicator, as one of the composer row's foldable actions.
 *
 *  `null` when the chat agent has written nothing to show. Reads
 *  `meta.latestTodoList` (projected in `handleEvent`) so the render path is
 *  O(1), with no walk of the events Map per `threadMap` flush.
 *
 *  Folded, the row button is gone and the menu row is the whole control. It
 *  carries the same words the button's accessible name does, which is the only
 *  channel a menu row has: colour and a pulse do not survive the fold. */
export function todoIndicatorAction(): HeaderActionSpec | null {
  const id = focusedThreadId.value;
  const meta = id ? threadMap.value.get(id)?.meta : undefined;
  const items = meta?.latestTodoList ?? null;
  const notes = meta?.latestTodoNotes ?? null;
  const summary = todoIndicatorSummary(items, notes);
  if (!summary) return null;
  return {
    key: 'todo-indicator',
    dataRole: 'todo-indicator',
    label: summary.menuLabel,
    tooltip: summary.tooltip,
    icon: () => <TodoListIcon />,
    render: (attrs) => todoListIndicatorBody({
      items,
      notes,
      attrs,
      // Re-pressing the button closes, which is the toggle the anchor exemption
      // in <Overlay> leaves to the control's own handler.
      onClick: (e) => {
        const self = e.currentTarget as HTMLElement;
        todoPanelAnchor.value = todoPanelAnchor.value ? null : self;
      },
    }),
    onMenuClick: (anchor) => { todoPanelAnchor.value = anchor; },
  };
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
