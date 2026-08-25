import { useState, useRef, useEffect } from 'preact/hooks';
import type { TriggerGroup } from '../../store/types';
import { collapsedTriggerGroupIds, toggleTriggerGroupCollapsed } from '../../store/store';
import { deleteTriggerGroup, renameTriggerGroup } from '../../store/actions/triggerGroups';
import { EditIcon, TrashIcon } from '../shared/icons';
import { PROSE_TEXT_ATTRS } from '../../utils/noAutofill';

interface Props {
  group: TriggerGroup;
}

/** Section header for one trigger group in the panel.
 *
 *  - Chevron toggles the per-device collapsed state (localStorage-backed).
 *  - Member-count badge shows how many triggers are assigned.
 *  - Inline rename: the rename button reveals an edit field over the name.
 *  - Delete: refused when non-empty; the action handler surfaces the toast.
 */
export function TriggerGroupHeader({ group }: Props) {
  const collapsed = collapsedTriggerGroupIds.value.has(group.id);
  const [editing, setEditing] = useState(false);
  const [draft, setDraft] = useState(group.name);
  // Enter and blur BOTH call commit; committing hides the input, whose trailing
  // blur would rename a second time. Guard with a submit-once flag, reset when
  // editing reopens.
  const renamingRef = useRef(false);
  const inputRef = useRef<HTMLInputElement>(null);

  // Leaving edit mode leaves the field focused (Enter commits without blurring,
  // and the field stays mounted now), so it would keep the mobile keyboard up
  // over a panel with nothing to type into. Same reason ThreadTitleEditor does
  // this. onBlur is only wired while editing, so this cannot re-commit.
  useEffect(() => {
    if (!editing) inputRef.current?.blur();
  }, [editing]);

  // While not editing, the field mirrors the served name. Without this a
  // `TriggerGroupRenamed` frame from another device leaves the old name behind
  // the rename button, and the next tap offers it back for editing. It also
  // resets the draft on every close, so an abandoned edit, an empty one and a
  // failed rename all settle on the stored name. Same shape as
  // ThreadTitleEditor (ADR 0118).
  useEffect(() => {
    if (!editing) setDraft(group.name);
  }, [group.name, editing]);

  async function commit() {
    if (renamingRef.current) return;
    const trimmed = draft.trim();
    if (!trimmed || trimmed === group.name) { setEditing(false); return; }
    renamingRef.current = true;
    await renameTriggerGroup(group.id, trimmed);
    setEditing(false);
  }

  return (
    <div
      class={`trigger-group-header${collapsed ? ' trigger-group-collapsed' : ''}${editing ? ' trigger-group-renaming' : ''}`}
    >
      <button
        class="trigger-group-toggle"
        type="button"
        onClick={() => toggleTriggerGroupCollapsed(group.id)}
        aria-expanded={!collapsed}
        data-tooltip={collapsed ? 'Expand' : 'Collapse'}
      >
        <span class="trigger-group-chevron">{collapsed ? '▸' : '▾'}</span>
        {/* The field is MOUNTED whether or not we are editing, transparent and
            pointer-inert over the name until then. iOS opens the keyboard only
            for a focus() that happens inside the user's gesture, and a field
            conditionally rendered by the tap does not exist yet at that moment:
            focusing it on the next render (autoFocus, or an effect) lands after
            the gesture has ended, so the field appeared with no keyboard. The
            rename button focuses this one directly instead. */}
        <span class="trigger-group-name-slot">
          <span class="trigger-group-name">{group.name}</span>
          <input
            ref={inputRef}
            class="trigger-group-name-input"
            type="text"
            value={draft}
            {...PROSE_TEXT_ATTRS}
            tabIndex={editing ? 0 : -1}
            // Transparent and pointer-inert is not hidden: without this every
            // heading would offer a screen reader a textbox that does nothing,
            // and the toggle's name would read the group twice, since a button
            // takes its name from its content and an embedded control
            // contributes its VALUE. Flips with `editing`, so the field is
            // exposed exactly while it is real. Safe against the
            // aria-hidden-on-a-focused-element trap: nothing but the rename
            // button can reach it (tabIndex -1, pointer-events none), and that
            // button unhides it in the same tap that focuses it.
            aria-hidden={!editing}
            onClick={e => e.stopPropagation()}
            onInput={e => setDraft((e.target as HTMLInputElement).value)}
            onBlur={editing ? commit : undefined}
            onKeyDown={e => {
              if (e.key === 'Enter') void commit();
              else if (e.key === 'Escape') setEditing(false);
            }}
          />
        </span>
        <span class="trigger-group-count">({group.member_count})</span>
      </button>
      <div class="trigger-group-actions">
        <button
          class="icon-btn row-icon trigger-group-rename"
          type="button"
          onClick={e => {
            e.stopPropagation();
            renamingRef.current = false;
            setDraft(group.name);
            // Synchronous, and before the state flip: this call is what the
            // keyboard rides in on, so it has to happen while the tap is still
            // the current gesture. Preact flushes the render afterwards, in a
            // microtask that no longer carries one.
            inputRef.current?.focus();
            inputRef.current?.select();
            setEditing(true);
          }}
          aria-label={`Rename group “${group.name}”`}
          data-tooltip="Rename group"
        >
          <EditIcon />
        </button>
        <button
          class="icon-btn row-icon trigger-group-delete"
          type="button"
          onClick={e => { e.stopPropagation(); void deleteTriggerGroup(group.id, group.name); }}
          aria-label={`Delete group “${group.name}”`}
          data-tooltip={group.member_count > 0 ? 'Move triggers out first' : 'Delete group'}
          disabled={group.member_count > 0}
        >
          <TrashIcon />
        </button>
      </div>
    </div>
  );
}
