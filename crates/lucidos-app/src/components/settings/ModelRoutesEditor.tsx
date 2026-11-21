import { useState } from 'preact/hooks';
import { Dropdown } from '../shared/Dropdown';
import { ChevronDownIcon, ChevronUpIcon, TrashIcon } from '../shared/icons';
import { PROVIDERS, providerLabel } from '../../store/models';
import { routeDrafts, saveModelRoutes, type RouteDraft } from '../../store/actions/models';
import type { ModelInfo } from '../../api/types';

/** Move the item at `from` by `step`, returning a new array. */
function moved<T>(items: readonly T[], from: number, step: -1 | 1): T[] {
  const to = from + step;
  if (to < 0 || to >= items.length) return [...items];
  const next = [...items];
  [next[from], next[to]] = [next[to], next[from]];
  return next;
}

/** Settings → Models: edit which backends serve one model, in the order the
 *  engine tries them, and which of them it remembers as picked.
 *
 *  Builtins take route edits too. Which backends serve a model is a fact the
 *  vendor can change, not part of the row's identity. */
export function ModelRoutesEditor({ model, onClose }: { model: ModelInfo; onClose: () => void }) {
  const [drafts, setDrafts] = useState<RouteDraft[]>(() => routeDrafts(model));
  // `undefined` until the user picks, so saving never replays a stale seed.
  const [preferred, setPreferred] = useState<string | undefined>(undefined);
  const shownPreferred = preferred ?? model.preferred_provider ?? '';
  const unused = PROVIDERS.filter((p) => !drafts.some((d) => d.provider === p.value));

  function update(index: number, patch: Partial<RouteDraft>) {
    setDrafts(drafts.map((d, i) => (i === index ? { ...d, ...patch } : d)));
  }

  async function save() {
    const pick = preferred === undefined ? undefined : preferred || null;
    if (await saveModelRoutes(model.id, drafts, pick)) onClose();
  }

  return (
    <div class="model-routes-editor">
      <span class="list-row-details list-row-details-prose">
        The engine uses the first backend that is set up, unless one is
        remembered. Leave the id blank to send <code>{model.id}</code>, and the
        window blank to infer it from the id.
      </span>
      {drafts.map((draft, i) => (
        <div class="model-route-row" key={`${draft.provider}-${i}`}>
          <Dropdown
            options={PROVIDERS.filter(
              (p) => p.value === draft.provider || !drafts.some((d) => d.provider === p.value),
            )}
            value={draft.provider}
            onChange={(provider) => update(i, { provider })}
          />
          <input
            class="settings-text-input"
            aria-label={`Id sent to ${providerLabel(draft.provider)}`}
            placeholder={model.id}
            value={draft.id}
            onInput={(e) => update(i, { id: (e.currentTarget as HTMLInputElement).value })}
          />
          <input
            class="settings-text-input model-route-window"
            aria-label={`Context window on ${providerLabel(draft.provider)}`}
            inputMode="numeric"
            placeholder="Window"
            value={draft.contextWindow}
            onInput={(e) =>
              update(i, { contextWindow: (e.currentTarget as HTMLInputElement).value })
            }
          />
          <button
            class="icon-btn"
            aria-label={`Try ${providerLabel(draft.provider)} earlier`}
            disabled={i === 0}
            onClick={() => setDrafts(moved(drafts, i, -1))}
          >
            <ChevronUpIcon size="1rem" />
          </button>
          <button
            class="icon-btn"
            aria-label={`Try ${providerLabel(draft.provider)} later`}
            disabled={i === drafts.length - 1}
            onClick={() => setDrafts(moved(drafts, i, 1))}
          >
            <ChevronDownIcon size="1rem" />
          </button>
          <button
            class="icon-btn"
            aria-label={`Remove the ${providerLabel(draft.provider)} route`}
            disabled={drafts.length === 1}
            onClick={() => setDrafts(drafts.filter((_, j) => j !== i))}
          >
            <TrashIcon />
          </button>
        </div>
      ))}
      <div class="settings-row">
        <span class="settings-row-label">Remembered</span>
        <Dropdown
          options={[
            { value: '', label: 'None: first set up' },
            ...drafts.map((d) => ({ value: d.provider, label: providerLabel(d.provider) })),
          ]}
          value={drafts.some((d) => d.provider === shownPreferred) ? shownPreferred : ''}
          onChange={setPreferred}
        />
      </div>
      <div class="settings-row-options">
        <button
          class="action-btn"
          disabled={unused.length === 0}
          onClick={() =>
            setDrafts([...drafts, { provider: unused[0].value, id: '', contextWindow: '' }])
          }
        >
          Add route
        </button>
        <button class="action-btn" onClick={onClose}>
          Cancel
        </button>
        <button class="action-btn action-btn-confirm" onClick={() => void save()}>
          Save
        </button>
      </div>
    </div>
  );
}
