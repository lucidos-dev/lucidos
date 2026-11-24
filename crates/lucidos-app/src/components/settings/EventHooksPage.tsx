import { useEffect, useState } from 'preact/hooks';
import {
  armEventHook,
  disarmEventHook,
  fetchEventHooks,
  removeEventHook,
  type EventHook,
} from '../../api/client';
import { LoadingFade } from '../shared/LoadingFade';
import { ListSkeletonOf, SkBlock, SkText } from '../shared/Skeleton';
import { LoadableError } from '../shared/LoadableError';
import { eventHooksVersion, showConfirm, showToast } from '../../store/store';
import { toFailed, loadingIfFresh, type Loadable } from '../../store/types';
import { useDelayedLoading } from '../../hooks/useDelayedLoading';
import { useVersionedRefresh } from '../../hooks/useVersionedRefresh';
import { usePanelRefresh } from '../../hooks/usePanelRefresh';

export type EventHookState = 'Disarmed' | 'Armed' | 'Changed' | 'Missing';

export function hookState(hook: EventHook): EventHookState {
  if (hook.status === 'missing') return 'Missing';
  switch (hook.arm_state) {
    case 'disarmed': return 'Disarmed';
    case 'armed': return 'Armed';
    case 'changed': return 'Changed';
  }
}

function definitionOf(hook: EventHook) {
  return hook.status === 'missing' ? null : hook.definition;
}

export function hookTitle(hook: EventHook): string {
  return definitionOf(hook)?.name ?? hook.slug;
}

function hookEventType(hook: EventHook): string | null {
  return definitionOf(hook)?.event_type ?? hook.armed?.event_type ?? null;
}

export function hookVerbs(hook: EventHook): { arm: boolean; disarm: boolean } {
  const state = hookState(hook);
  return {
    arm: state === 'Disarmed' || state === 'Changed',
    disarm: hook.armed !== null,
  };
}

export function staleArmingLine(hook: EventHook): string | null {
  switch (hookState(hook)) {
    case 'Changed':
      return 'It changed since it was armed, so Lucidos refuses its events until you arm it again or disarm it.';
    case 'Missing':
      return 'Its folder is gone from data/hooks/, so Lucidos refuses its events until you disarm or remove it.';
    default:
      return null;
  }
}

export function confirmArming(hook: EventHook): Promise<boolean> {
  return showConfirm(
    `Arm the event hook "${hookTitle(hook)}"?\n\n` +
      `From then on, Lucidos asks its script before it records each ${hookEventType(hook)} ` +
      'event, and the script can stop that event. Arming binds its current definition and ' +
      'script, so an edit leaves it changed until you arm it again.',
    'Arm',
    { variant: 'default' },
  );
}

export function confirmRemoval(hook: EventHook): Promise<boolean> {
  if (hook.status === 'missing') {
    return showConfirm(
      `Remove the event hook "${hookTitle(hook)}"?\n\nIts folder is gone already, so this ends its arming.`,
      'Remove',
      { variant: 'default' },
    );
  }
  return showConfirm(
    `Remove the event hook "${hookTitle(hook)}"?\n\n` +
      `Lucidos deletes its folder, data/hooks/${hook.slug}/, and commits that. Any arming of it ends.`,
    'Remove',
    { variant: 'danger' },
  );
}

function failed(e: unknown) {
  showToast(e instanceof Error ? e.message : 'That did not work', 'error');
}

function EventHookRow({ hook, onChanged = () => {} }: { hook?: EventHook; onChanged?: () => void }) {
  const [busy, setBusy] = useState(false);
  const verbs = hook ? hookVerbs(hook) : { arm: false, disarm: false };
  const eventType = hook ? hookEventType(hook) : null;
  const staleArming = hook ? staleArmingLine(hook) : null;
  const refusal = verbs.arm ? hook?.arm_refusal ?? null : null;
  const removeRefusal = hook?.remove_refusal ?? null;

  async function act(verb: (hook: EventHook) => Promise<void>, confirm?: (hook: EventHook) => Promise<boolean>) {
    if (!hook || (confirm && !(await confirm(hook)))) return;
    setBusy(true);
    try {
      await verb(hook);
    } catch (e) {
      failed(e);
    } finally {
      setBusy(false);
      onChanged();
    }
  }

  return (
    <div class="list-row">
      <div class="list-row-info">
        <SkText class="title" as="div" w="9rem">{hook && hookTitle(hook)}</SkText>
        <SkText class="list-row-details" as="div" w="14rem">
          {hook && (
            <>
              <span>{hookState(hook)}</span>
              {eventType && <span>{eventType}</span>}
              <span><code>{hook.slug}</code></span>
            </>
          )}
        </SkText>
        {staleArming && (
          <div class="list-row-details list-row-details-prose error-text">{staleArming}</div>
        )}
        {hook?.status === 'invalid' && hook.problems.map((problem) => (
          <div key={problem} class="list-row-details list-row-details-prose error-text">{problem}</div>
        ))}
        {refusal && <div class="list-row-details list-row-details-prose">{refusal}</div>}
        {removeRefusal && <div class="list-row-details list-row-details-prose">{removeRefusal}</div>}
      </div>
      <div class="list-row-actions">
        <SkBlock w="4.5rem" h="2rem" round>
          <button
            class="action-btn action-btn-danger"
            disabled={busy || !!removeRefusal}
            onClick={() => void act(removeEventHook, confirmRemoval)}
          >
            Remove
          </button>
        </SkBlock>
        {hook && verbs.disarm && (
          <button
            class="action-btn"
            disabled={busy}
            onClick={() => void act(disarmEventHook)}
          >
            Disarm
          </button>
        )}
        {hook && verbs.arm && (
          <button
            class="action-btn action-btn-confirm"
            disabled={busy || !!refusal}
            onClick={() => void act(armEventHook, confirmArming)}
          >
            Arm
          </button>
        )}
      </div>
    </div>
  );
}

export function EventHooksPage() {
  const [loadable, setLoadable] = useState<Loadable<EventHook[]>>({ status: 'not-loaded' });
  const showLoading = useDelayedLoading(loadable);

  function reload(): Promise<void> {
    setLoadable(loadingIfFresh);
    return fetchEventHooks()
      .then((rows) => setLoadable({ status: 'loaded', data: rows }))
      .catch((e: unknown) => setLoadable(toFailed(e)));
  }

  useEffect(() => { void reload(); }, []);
  usePanelRefresh('event-hooks', reload);
  useVersionedRefresh(eventHooksVersion.value, false, reload);

  return (
    <div class="settings-section">
      <p class="settings-section-desc">
        Scripts Lucidos asks before it records an event, so the workspace can stop
        progress until its own rule holds. Each is a folder under data/hooks/. A hook
        runs only once you arm it, and an edit leaves it changed until you arm it again.
      </p>
      {loadable.status === 'failed' && <LoadableError noun="event hooks" error={loadable.error} />}
      <LoadingFade
        showSkeleton={showLoading}
        skeleton={<ListSkeletonOf count={2} containerClass="list-rows" row={() => <EventHookRow />} />}
      >
        {loadable.status === 'loaded' && (
          <div class="list-rows">
            {loadable.data.length === 0 && <div class="empty-state">No event hooks yet.</div>}
            {loadable.data.map((hook) => (
              <EventHookRow key={hook.slug} hook={hook} onChanged={reload} />
            ))}
          </div>
        )}
      </LoadingFade>
    </div>
  );
}
