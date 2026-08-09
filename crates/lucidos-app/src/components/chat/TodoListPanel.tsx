import { useSignal } from '@preact/signals';
import { useRef, useState } from 'preact/hooks';
import type { Ref } from 'preact';
import { useAnchoredPosition } from '../../hooks/useAnchoredPopover';
import { focusedThreadId, threadMap } from '../../store/store';
import type { TodoItem, TodoStatus } from '../../store/thread-events';
import { CloseIcon, TodoListIcon } from '../shared/icons';
import { Overlay } from '../shared/Overlay';

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
  const waiting = items.filter((i) => i.status === 'waiting').length;
  const abandoned = items.filter((i) => i.status === 'abandoned').length;
  const inProgress = items.find((i) => i.status === 'in_progress');
  // Four honest indicator states, stamped as `data-state` and distinguished
  // by COLOR alone (todo-list.css) over one shared ticked-checkbox glyph:
  //   - in-progress: the agent is actively working an item (accent)
  //   - waiting: no in-progress item AND at least one item is parked on a live
  //     event wait, i.e. the agent stopped on purpose and something will wake
  //     it (accent-yellow, live but not working)
  //   - abandoned: no in-progress item AND at least one item was abandoned (dimmed)
  //   - idle: every non-completed item is gone (all done, or nothing pending)
  // Waiting outranks abandoned because it is the live fact: a list carrying
  // both has parked items that are still going somewhere.
  const state = inProgress
    ? 'in-progress'
    : waiting > 0
      ? 'waiting'
      : abandoned > 0
        ? 'abandoned'
        : 'idle';
  const tooltip = inProgress
    ? inProgress.active_form
    : waiting > 0
      ? `${completed} of ${total} done, ${waiting} waiting`
      : abandoned > 0
        ? `${completed} of ${total} done, ${abandoned} abandoned`
        : `${completed} of ${total} done`;
  // The aria-label names the state, it does not just count. Color is the ONLY
  // visual channel carrying it now that all four states share one glyph, and
  // color is exactly the channel a screen reader cannot read and forced-colors
  // mode overwrites (there --accent, --accent-yellow and --text-secondary all
  // collapse to the system foreground, so the states would be identical).
  // The tooltip can't stand in for it: it is desktop-hover only.
  const ariaLabel = inProgress
    ? `Todo list: ${inProgress.active_form}. ${completed} of ${total} done. Click to expand.`
    : waiting > 0
      ? `Todo list: ${completed} of ${total} done, ${waiting} waiting. Click to expand.`
      : abandoned > 0
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
      <TodoListIcon />
    </button>
  );
}

/** One marker glyph per status, all from the same geometric-circle family so
 *  the 1rem marker column reads as one column. `waiting`'s clock face echoes
 *  the *subscription indicator*'s own clock icon beside it in the prompt bar:
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
  onClose,
}: {
  items: TodoItem[];
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

/** Symmetric to `CodingAgentControlMenu`: mounted in the prompt-bar actions row,
 *  hidden when the chat agent hasn't written a list. Reads from
 *  `meta.latestTodoList` (projected in `handleEvent`) so the render path
 *  is O(1) — no walk of the events Map per threadMap flush. */
export function TodoListIndicator() {
  const open = useSignal(false);
  // useState (not useRef) so the dismiss hook re-runs once the button mounts
  // and we have a real anchor to exclude from the outside-click test.
  const [anchorEl, setAnchorEl] = useState<HTMLButtonElement | null>(null);
  const panelRef = useRef<HTMLDivElement>(null);

  const id = focusedThreadId.value;
  const items = id ? threadMap.value.get(id)?.meta.latestTodoList ?? null : null;
  const isOpen = open.value && !!items && items.length > 0;
  const pos = useAnchoredPosition(isOpen ? anchorEl : null, panelRef, '.thread-pane');

  return (
    <>
      {todoListIndicatorBody({
        items,
        onClick: () => (open.value = !open.value),
        buttonRef: setAnchorEl,
      })}
      {/* The Overlay panel IS the `.todo-panel` box now, placed by
          `useAnchoredPosition` rather than by CSS, and portaled because the
          composer's ancestors animate `transform`. Anchor is the indicator
          button. Same wiring as the subscription popover beside it; the
          contract lives in <Overlay>. */}
      <Overlay
        open={isOpen}
        onClose={() => (open.value = false)}
        anchor={anchorEl}
        backdrop={false}
        portal
        panelClass="prompt-bar-popover todo-panel"
        // `--prompt-bar-popover-fit` is the thread pane's usable width, the box
        // the hook clamped this panel's position into (see EventWaitPanel).
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
        {items && items.length > 0 && todoListPanelBody({ items, onClose: () => (open.value = false) })}
      </Overlay>
    </>
  );
}
