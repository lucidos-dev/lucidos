import type { TriggerGroup } from '../../store/types';
import { collapsedTriggerGroupIds, toggleTriggerGroupCollapsed } from '../../store/store';
import { deleteTriggerGroup, renameTriggerGroup } from '../../store/actions/triggerGroups';
import { useInlineRename } from '../../hooks/useInlineRename';
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
 *  - Delete: always live. A non-empty group is refused by the server, and the
 *    action handler surfaces that refusal as a toast.
 */
export function TriggerGroupHeader({ group }: Props) {
  const collapsed = collapsedTriggerGroupIds.value.has(group.id);
  const {
    renaming: editing,
    draft,
    setDraft,
    inputRef,
    open,
    commit,
    cancel,
  } = useInlineRename(group.name, (next) => renameTriggerGroup(group.id, next));

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
            // The blur commits, so the central Escape policy must not blur it.
            data-escape-self
            onKeyDown={e => {
              if (e.key === 'Enter') void commit();
              else if (e.key === 'Escape') { e.preventDefault(); cancel(); }
            }}
          />
        </span>
        <span class="trigger-group-count">({group.member_count})</span>
      </button>
      <div class="trigger-group-actions">
        <button
          class="icon-btn row-icon trigger-group-rename"
          type="button"
          onClick={e => { e.stopPropagation(); open(); }}
          aria-label={`Rename group “${group.name}”`}
          data-tooltip="Rename group"
        >
          <EditIcon />
        </button>
        {/* Never `disabled`, however many triggers the group holds. ADR 0168:
            `.icon-btn:disabled` sets `pointer-events: none`, so the tooltip that
            states the block is the one thing a disabled button cannot show, on
            hover or on long press. The button stays live and the server owns the
            refusal, which `deleteTriggerGroup` reports as a toast naming the
            count. Same rule as the change actions, pinned by
            `changes/__tests__/no-disabled-change-action.test.tsx`. */}
        <button
          class="icon-btn row-icon trigger-group-delete"
          type="button"
          onClick={e => { e.stopPropagation(); void deleteTriggerGroup(group.id, group.name); }}
          aria-label={`Delete group “${group.name}”`}
          data-tooltip={group.member_count > 0 ? 'Move triggers out first' : 'Delete group'}
        >
          <TrashIcon />
        </button>
      </div>
    </div>
  );
}
