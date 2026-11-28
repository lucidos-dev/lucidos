import { useState, useRef, useEffect } from 'preact/hooks';
import {
  triggers,
  triggerGroups,
  collapsedTriggerSectionIds,
  expandTriggerSection,
  triggerScrollTarget,
  showToast,
  UNGROUPED_TRIGGER_SECTION_ID,
} from '../../store/store';
import { loadTriggers, openAddTrigger } from '../../store/actions/triggers';
import { createTriggerGroup, loadTriggerGroups } from '../../store/actions/triggerGroups';
import { usePanelRefresh } from '../../hooks/usePanelRefresh';
import { useDelayedLoading } from '../../hooks/useDelayedLoading';
import { hasNoMoreRuns, loadedOr } from '../../store/types';
import type { TriggerInfo } from '../../store/types';
import { TriggerItem } from './TriggerItem';
import { TriggerGroupHeader, UngroupedHeader } from './TriggerGroupHeader';
import { LoadableError } from '../shared/LoadableError';
import { ListRowAddCard } from '../shared/ListRowAddCard';
import { ListSkeletonOf } from '../shared/Skeleton';
import { LoadingFade } from '../shared/LoadingFade';
import { Disclosure } from '../shared/Disclosure';
import { applyNavFocus } from '../shared/focusMarker';
import { resolveTriggerScrollStep } from './triggerScrollStep';
import { PROSE_TEXT_ATTRS } from '../../utils/noAutofill';
import { scrollBehavior } from '../../utils/motion';

const NO_COLLAPSED_SECTIONS: ReadonlySet<string> = new Set();

function sortByCompletion(a: TriggerInfo, b: TriggerInfo): number {
  const aNoMore = hasNoMoreRuns(a);
  const bNoMore = hasNoMoreRuns(b);
  if (aNoMore !== bNoMore) return aNoMore ? 1 : -1;
  return 0;
}

function refreshTriggersPanel(): Promise<unknown> {
  return Promise.all([
    loadTriggers(),
    loadTriggerGroups(),
  ]);
}

export function TriggersView() {
  usePanelRefresh('triggers', refreshTriggersPanel);
  const triggersLoadable = triggers.value;
  const groupsLoadable = triggerGroups.value;
  const showTriggersLoading = useDelayedLoading(triggersLoadable);
  const listRef = useRef<HTMLDivElement>(null);
  const [newGroupName, setNewGroupName] = useState<string | null>(null);
  const groupDraftSettledRef = useRef(false);

  async function commitNewGroup() {
    if (groupDraftSettledRef.current) return;
    const trimmed = (newGroupName ?? '').trim();
    if (!trimmed) { setNewGroupName(null); return; }
    groupDraftSettledRef.current = true;
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
      <div class="list-rows list-rows-divided" ref={listRef}>
        <LoadingFade showSkeleton={showTriggersLoading} skeleton={<ListSkeletonOf fill containerClass="trigger-group-section" row={() => <TriggerItem />} />}>
          {triggersLoadable.status === 'loaded' ? (
            <TriggersLoaded
              triggersData={triggersLoadable.data}
              groupsLoadable={groupsLoadable}
              newGroupName={newGroupName}
              setNewGroupName={setNewGroupName}
              commitNewGroup={commitNewGroup}
              groupDraftSettledRef={groupDraftSettledRef}
              listRef={listRef}
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
  groupDraftSettledRef,
  listRef,
}: {
  triggersData: TriggerInfo[];
  groupsLoadable: typeof triggerGroups.value;
  newGroupName: string | null;
  setNewGroupName: (v: string | null) => void;
  commitNewGroup: () => void;
  groupDraftSettledRef: { current: boolean };
  listRef: { current: HTMLDivElement | null };
}) {
  const groups = loadedOr(groupsLoadable, []);
  const knownGroupIds = new Set(groups.map(g => g.id));

  const sectionOf = (t: TriggerInfo) =>
    t.group_id && knownGroupIds.has(t.group_id) ? t.group_id : UNGROUPED_TRIGGER_SECTION_ID;
  const sectionRows = triggersData.map(t => ({ id: t.id, section: sectionOf(t) }));
  const bySection = new Map<string, TriggerInfo[]>();
  for (const t of triggersData) {
    const key = sectionOf(t);
    const bucket = bySection.get(key);
    if (bucket) bucket.push(t);
    else bySection.set(key, [t]);
  }
  for (const bucket of bySection.values()) bucket.sort(sortByCompletion);

  const collapsed = groups.length > 0 ? collapsedTriggerSectionIds.value : NO_COLLAPSED_SECTIONS;
  const ungroupedTriggers = bySection.get(UNGROUPED_TRIGGER_SECTION_ID) ?? [];
  const ungroupedCollapsed = collapsed.has(UNGROUPED_TRIGGER_SECTION_ID);

  useEffect(() => {
    const step = resolveTriggerScrollStep(triggerScrollTarget.value, sectionRows, collapsed);
    if (step.kind === 'idle') return;
    if (step.kind === 'drop') {
      triggerScrollTarget.value = null;
      return;
    }
    if (step.kind === 'expand') {
      expandTriggerSection(step.sectionId);
      return;
    }
    const el = listRef.current?.querySelector<HTMLElement>(
      `[data-trigger-id="${CSS.escape(step.triggerId)}"]`,
    );
    if (!el) return;
    el.scrollIntoView({ block: 'center', behavior: scrollBehavior() });
    applyNavFocus(el);
    triggerScrollTarget.value = null;
  }, [triggerScrollTarget.value, triggersData, groupsLoadable, collapsed, listRef]);

  const creatingGroup = newGroupName !== null;
  const createInputRef = useRef<HTMLInputElement>(null);
  useEffect(() => {
    if (!creatingGroup) createInputRef.current?.blur();
  }, [creatingGroup]);

  return (
    <>
      {groups.map(group => {
        const members = bySection.get(group.id) ?? [];
        const isCollapsed = collapsed.has(group.id);
        return (
          <div class="trigger-group-section" key={group.id}>
            <TriggerGroupHeader group={group} />
            <Disclosure
              open={!isCollapsed}
              instant={triggerScrollTarget.value !== null}
              bodyClass="trigger-group-members"
            >
              {members.map(trigger => (
                <TriggerItem key={trigger.id} trigger={trigger} />
              ))}
            </Disclosure>
          </div>
        );
      })}
      {ungroupedTriggers.length > 0 && (
        <div class="trigger-group-section trigger-group-section-ungrouped">
          {groups.length > 0 && (
            <UngroupedHeader count={ungroupedTriggers.length} collapsed={ungroupedCollapsed} />
          )}
          <Disclosure
            open={!ungroupedCollapsed}
            instant={triggerScrollTarget.value !== null}
            bodyClass="trigger-group-members"
          >
            {ungroupedTriggers.map(trigger => (
              <TriggerItem key={trigger.id} trigger={trigger} />
            ))}
          </Disclosure>
        </div>
      )}
      <div class={`trigger-group-create-row${creatingGroup ? '' : ' trigger-group-create-idle'}`}>
        <input
          ref={createInputRef}
          class="trigger-group-name-input"
          type="text"
          value={newGroupName ?? ''}
          placeholder="Group name"
          {...PROSE_TEXT_ATTRS}
          tabIndex={creatingGroup ? 0 : -1}
          aria-hidden={!creatingGroup}
          onInput={e => setNewGroupName((e.target as HTMLInputElement).value)}
          onBlur={creatingGroup ? commitNewGroup : undefined}
          data-escape-self
          onKeyDown={e => {
            if (e.key === 'Enter') void commitNewGroup();
            else if (e.key === 'Escape') {
              e.preventDefault();
              groupDraftSettledRef.current = true;
              setNewGroupName(null);
              e.currentTarget.blur();
            }
          }}
        />
      </div>
      <div class="trigger-add-row">
        <ListRowAddCard label="Add Trigger" onClick={openAddTrigger} />
        <ListRowAddCard
          label="New Group"
          onClick={() => {
            groupDraftSettledRef.current = false;
            createInputRef.current?.focus();
            setNewGroupName('');
          }}
        />
      </div>
    </>
  );
}
