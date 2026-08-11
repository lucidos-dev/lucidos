import { useState, useRef, useEffect } from 'preact/hooks';
import { triggers, triggerGroups, collapsedTriggerGroupIds, showToast } from '../../store/store';
import { openAddTrigger } from '../../store/actions/triggers';
import { createTriggerGroup } from '../../store/actions/triggerGroups';
import { useDelayedLoading } from '../../hooks/useDelayedLoading';
import { hasNoMoreRuns, loadedOr } from '../../store/types';
import type { TriggerInfo } from '../../store/types';
import { TriggerItem } from './TriggerItem';
import { TriggerGroupHeader } from './TriggerGroupHeader';
import { LoadableError } from '../shared/LoadableError';
import { ListRowAddCard } from '../shared/ListRowAddCard';
import { ListSkeletonOf } from '../shared/Skeleton';
import { LoadingFade } from '../shared/LoadingFade';

const UNGROUPED_KEY = '__ungrouped__';

function sortByCompletion(a: TriggerInfo, b: TriggerInfo): number {
  // Stopped triggers sink to the bottom within their section, preserving the
  // panel's pre-grouping behavior.
  const aNoMore = hasNoMoreRuns(a);
  const bNoMore = hasNoMoreRuns(b);
  if (aNoMore !== bNoMore) return aNoMore ? 1 : -1;
  return 0;
}

export function TriggersView() {
  const triggersLoadable = triggers.value;
  const groupsLoadable = triggerGroups.value;
  const showTriggersLoading = useDelayedLoading(triggersLoadable);
  const [newGroupName, setNewGroupName] = useState<string | null>(null);
  // Enter and blur BOTH call commitNewGroup; committing unmounts the field,
  // which fires a trailing blur that would POST the same name again (409
  // "already exists", a sticky error toast). Guard with a submit-once flag,
  // reset when the field is reopened.
  const creatingGroupRef = useRef(false);

  async function commitNewGroup() {
    if (creatingGroupRef.current) return;
    const trimmed = (newGroupName ?? '').trim();
    if (!trimmed) { setNewGroupName(null); return; }
    creatingGroupRef.current = true;
    const group = await createTriggerGroup(trimmed);
    if (group) showToast(`Group "${group.name}" created`, 'info');
    setNewGroupName(null);
  }

  if (triggersLoadable.status === 'failed') {
    return (
      <div class="content-view active">
        <div class="list-rows">
          <LoadableError noun="triggers" error={triggersLoadable.error} />
        </div>
      </div>
    );
  }

  return (
    <div class="content-view active">
      <div class="list-rows">
        <LoadingFade showSkeleton={showTriggersLoading} skeleton={<ListSkeletonOf fill containerClass="trigger-group-section" row={() => <TriggerItem />} />}>
          {triggersLoadable.status === 'loaded' ? (
            <TriggersLoaded
              triggersData={triggersLoadable.data}
              groupsLoadable={groupsLoadable}
              newGroupName={newGroupName}
              setNewGroupName={setNewGroupName}
              commitNewGroup={commitNewGroup}
              creatingGroupRef={creatingGroupRef}
            />
          ) : null}
        </LoadingFade>
      </div>
    </div>
  );
}

function TriggersLoaded({
  triggersData,
  groupsLoadable,
  newGroupName,
  setNewGroupName,
  commitNewGroup,
  creatingGroupRef,
}: {
  triggersData: TriggerInfo[];
  groupsLoadable: typeof triggerGroups.value;
  newGroupName: string | null;
  setNewGroupName: (v: string | null) => void;
  commitNewGroup: () => void;
  creatingGroupRef: { current: boolean };
}) {
  // Group registry is small; if it failed, fall back to a flat panel under
  // "Ungrouped" so the user still sees their triggers. The failure surfaces
  // separately wherever the registry is needed (e.g. the picker in the trigger
  // form). Same idea as triggers' empty-as-failed guard.
  const groups = loadedOr(groupsLoadable, []);
  const knownGroupIds = new Set(groups.map(g => g.id));

  // Bucket triggers by group id (null → ungrouped). A trigger whose group_id
  // doesn't resolve to a known group (e.g. concurrent delete landed between
  // the trigger's group_id update and the panel refetch) falls back to the
  // Ungrouped section so the row can never go invisible.
  const byGroup = new Map<string, TriggerInfo[]>();
  for (const t of triggersData) {
    const key = t.group_id && knownGroupIds.has(t.group_id) ? t.group_id : UNGROUPED_KEY;
    const bucket = byGroup.get(key);
    if (bucket) bucket.push(t);
    else byGroup.set(key, [t]);
  }
  for (const bucket of byGroup.values()) bucket.sort(sortByCompletion);

  const collapsed = collapsedTriggerGroupIds.value;
  const ungroupedTriggers = byGroup.get(UNGROUPED_KEY) ?? [];

  const creatingGroup = newGroupName !== null;
  const createInputRef = useRef<HTMLInputElement>(null);
  // Closing the field leaves it focused (Escape and a committed Enter both do),
  // and it stays mounted now, so the mobile keyboard would hang over a panel
  // with nothing to type into. onBlur is only wired while open, so this cannot
  // re-submit the name.
  useEffect(() => {
    if (!creatingGroup) createInputRef.current?.blur();
  }, [creatingGroup]);

  return (
    <>
      {groups.map(group => {
        const members = byGroup.get(group.id) ?? [];
        const isCollapsed = collapsed.has(group.id);
        return (
          <div class="trigger-group-section" key={group.id}>
            <TriggerGroupHeader group={group} />
            {!isCollapsed && members.map(trigger => (
              <TriggerItem key={trigger.id} trigger={trigger} />
            ))}
          </div>
        );
      })}
      {ungroupedTriggers.length > 0 && (
        <div class="trigger-group-section trigger-group-section-ungrouped">
          {groups.length > 0 && (
            <div class="trigger-group-header trigger-group-header-ungrouped">
              <span class="trigger-group-name">Ungrouped</span>
              <span class="trigger-group-count">({ungroupedTriggers.length})</span>
            </div>
          )}
          {ungroupedTriggers.map(trigger => (
            <TriggerItem key={trigger.id} trigger={trigger} />
          ))}
        </div>
      )}
      {/* Mounted whether or not the field is open, clipped to nothing until it
          is, for the same reason as the group header's rename field: iOS raises
          the keyboard only for a focus() made inside the user's own gesture, so
          the field the New Group card focuses has to already exist when the card
          is tapped. The row collapses rather than unmounting, and the input
          inside keeps its own box, so the element iOS scrolls into view is a
          real one. */}
      <div class={`trigger-group-create-row${creatingGroup ? '' : ' trigger-group-create-idle'}`}>
        <input
          ref={createInputRef}
          class="trigger-group-name-input"
          type="text"
          value={newGroupName ?? ''}
          placeholder="Group name"
          tabIndex={creatingGroup ? 0 : -1}
          // Clipped is not hidden: without this a screen reader would find a
          // "Group name" field sitting in the panel at all times, doing
          // nothing. Flips with the open state, and the New Group button
          // unhides it in the same tap that focuses it.
          aria-hidden={!creatingGroup}
          onInput={e => setNewGroupName((e.target as HTMLInputElement).value)}
          onBlur={creatingGroup ? commitNewGroup : undefined}
          onKeyDown={e => {
            if (e.key === 'Enter') void commitNewGroup();
            else if (e.key === 'Escape') setNewGroupName(null);
          }}
        />
      </div>
      <div class="trigger-add-row">
        <ListRowAddCard label="Add Trigger" onClick={openAddTrigger} />
        <ListRowAddCard
          label="New Group"
          onClick={() => {
            creatingGroupRef.current = false;
            // Synchronous, and before the state flip: see the note above.
            createInputRef.current?.focus();
            setNewGroupName('');
          }}
        />
      </div>
    </>
  );
}
