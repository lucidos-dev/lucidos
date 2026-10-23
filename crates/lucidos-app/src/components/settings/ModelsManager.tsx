import { Fragment } from 'preact';
import { useState } from 'preact/hooks';
import { chatModels } from '../../store/store';
import { Dropdown } from '../shared/Dropdown';
import { LoadableError } from '../shared/LoadableError';
import { ListRowAddCard } from '../shared/ListRowAddCard';
import { Disclosure } from '../shared/Disclosure';
import { ToggleIconButton } from '../shared/ToggleIconButton';
import { LoadingFade } from '../shared/LoadingFade';
import { ListSkeletonOf, SkBlock, SkText } from '../shared/Skeleton';
import { useDelayedLoading } from '../../hooks/useDelayedLoading';
import { ChevronDownIcon, ChevronRightIcon, ImageIcon } from '../shared/icons';
import {
  setModelVision, deleteModel, submitNewModel, hasConfiguredRoute, resolvedRoute,
} from '../../store/actions/models';
import { IntelligenceLinkRow } from './IntelligencePage';
import { formatContextWindow } from '../../utils/formatTokens';
import { ModelRoutesEditor } from './ModelRoutesEditor';
import type { ModelInfo } from '../../api/types';
import { PROVIDERS } from '../../store/models';

/** The route that will serve this model, which is what the row's window
 *  describes. A window is per backend, so naming the row's first would be wrong
 *  the moment the preferred one declares a different one. */
function served(m: ModelInfo) {
  return resolvedRoute(m.id) ?? m.routes[0];
}

/** Which row's route editor is open, and how to open or close one. */
export interface RouteEditing {
  id: string | null;
  open: (id: string | null) => void;
}

/** One registry row: the model's name and id, then the metadata cluster, and
 *  the route editor under it when open. Shared by the current list and the
 *  collapsed Retired group, so a retired user model keeps its Delete. With no
 *  model, inside a `SkeletonProvider`, it is the list's loading placeholder. */
function modelRow(m?: ModelInfo, editing?: RouteEditing) {
  const editorOpen = !!m && editing?.id === m.id;
  return (
    <Fragment key={m?.id}>
      <div class="model-manager-row">
        <div class="model-manager-info">
          <SkText class="model-manager-name" as="div" w="8rem">{m?.label}</SkText>
          <SkText class="model-manager-id" as="div" w="13rem">
            {m && <>{m.id} · {formatContextWindow(served(m)?.context_window ?? null)}</>}
          </SkText>
        </div>
        {m ? modelMeta(m, editorOpen, editing) : (
          <div class="model-manager-meta"><SkBlock w="4.5rem" h="1.25rem" round /></div>
        )}
      </div>
      <Disclosure open={editorOpen}>
        {m && editing && <ModelRoutesEditor model={m} onClose={() => editing.open(null)} />}
      </Disclosure>
    </Fragment>
  );
}

/** A row's backend badges and actions. */
function modelMeta(m: ModelInfo, editorOpen: boolean, editing?: RouteEditing) {
  return (
    <div class="model-manager-meta">
      {/* One badge per backend that can serve this model, in the order the
          engine resolves them. The preferred one is marked, since a pick is
          honoured or refused rather than substituted: reading which backend
          a turn will reach is the point of the row. */}
      {m.routes.map((r) => (
        <span
          key={r.provider}
          class="model-provider-badge"
          data-preferred={r.provider === m.preferred_provider ? 'true' : undefined}
          data-tooltip={r.id === m.id ? undefined : `Sent as ${r.id}`}
        >
          {r.provider}
        </span>
      ))}
      {!hasConfiguredRoute(m) && (
        <span
          class="model-provider-badge model-provider-unconfigured"
          data-tooltip="No provider set up. This model stays out of the picker until you add one under Providers"
        >
          not set up
        </span>
      )}
      {/* The *vision flag*. Chat sends images and image description runs
          only on models that read images, so this is where a wrong seed gets
          fixed. */}
      <ToggleIconButton
        tone="glyph"
        class="model-vision-toggle"
        pressed={m.vision}
        aria-label={`${m.label} reads images`}
        data-tooltip={m.vision ? 'Reads images' : 'Text only. Tap if it reads images'}
        onClick={() => void setModelVision(m.id, !m.vision)}
      >
        <ImageIcon />
      </ToggleIconButton>
      {editing && (
        <button
          class="action-btn"
          aria-expanded={editorOpen}
          onClick={() => editing.open(editorOpen ? null : m.id)}
        >
          Routes
        </button>
      )}
      {m.source === 'user' ? (
        <button class="action-btn action-btn-danger" onClick={() => void deleteModel(m.id)}>
          Delete
        </button>
      ) : (
        <span class="list-row-details">builtin</span>
      )}
    </div>
  );
}

/** The registry, split into current models and retired ones.
 *
 *  The retired rows sit behind a disclosure because a retired builtin keeps its
 *  row forever: that is the whole reason routing still resolves a saved
 *  `chat_model`. Left flat, the page is mostly models nothing will run. They
 *  stay one tap away, with their routes and their Delete intact.
 *
 *  A pure function, so the grouping is testable without a DOM. The component
 *  owns the state; this owns the markup. */
export function modelManagerList(
  models: readonly ModelInfo[],
  showRetired: boolean,
  onToggleRetired: () => void,
  editing?: RouteEditing,
) {
  const current = models.filter((m) => m.successor === null);
  const retired = models.filter((m) => m.successor !== null);
  return (
    <>
      {current.map((m) => modelRow(m, editing))}
      {retired.length > 0 && (
        <>
          <button
            type="button"
            class="settings-disclosure-toggle"
            aria-expanded={showRetired}
            onClick={onToggleRetired}
          >
            {showRetired ? <ChevronDownIcon size="1rem" /> : <ChevronRightIcon size="1rem" />}
            Retired ({retired.length})
          </button>
          <Disclosure open={showRetired}>{retired.map((m) => modelRow(m, editing))}</Disclosure>
        </>
      )}
    </>
  );
}

/** Settings → Models manager: list every registry model with its provider
 *  badges (builtins are never deleted; user models can be), plus an inline
 *  "Add Model" form. Which models the picker lists is the Lucidos Agent's
 *  intelligence ladder, on Settings → Intelligence. */
export function ModelsManager() {
  const loadable = chatModels.value;
  const showLoading = useDelayedLoading(loadable);
  const [adding, setAdding] = useState(false);
  const [showRetired, setShowRetired] = useState(false);
  const [editingId, setEditingId] = useState<string | null>(null);
  const [id, setId] = useState('');
  const [label, setLabel] = useState('');
  const [provider, setProvider] = useState('anthropic');
  const [contextWindow, setContextWindow] = useState('');
  const [vision, setVision] = useState(false);

  async function add() {
    const ok = await submitNewModel(id, label, provider, contextWindow, vision);
    if (ok) {
      setId('');
      setLabel('');
      setProvider('anthropic');
      setContextWindow('');
      setVision(false);
      setAdding(false);
    }
  }

  return (
    <div class="settings-section">
      <div class="settings-section-title" data-search-anchor="models:manage">Manage models</div>
      <IntelligenceLinkRow>
        The model picker lists the models with a rung on the Lucidos Agent's intelligence ladder
      </IntelligenceLinkRow>
      {loadable.status === 'failed' && (
        <LoadableError noun="models" error={loadable.error} />
      )}
      <LoadingFade
        showSkeleton={showLoading}
        skeleton={<ListSkeletonOf count={4} row={() => modelRow()} />}
      >
        {loadable.status === 'loaded' && (
          <>
            {modelManagerList(loadable.data, showRetired, () => setShowRetired((prev) => !prev), {
              id: editingId,
              open: setEditingId,
            })}
            {adding ? (
              <div class="settings-section">
                <div class="settings-row">
                  <span class="settings-row-label">Model id</span>
                  <input
                    class="settings-text-input"
                    placeholder="claude-fable-5"
                    value={id}
                    onInput={(e) => setId((e.currentTarget as HTMLInputElement).value)}
                  />
                </div>
                <div class="settings-row">
                  <span class="settings-row-label">Label</span>
                  <input
                    class="settings-text-input"
                    placeholder="Fable 5"
                    value={label}
                    onInput={(e) => setLabel((e.currentTarget as HTMLInputElement).value)}
                  />
                </div>
                <div class="settings-row">
                  <span class="settings-row-label">Provider</span>
                  <Dropdown options={PROVIDERS} value={provider} onChange={setProvider} />
                </div>
                <div class="settings-row">
                  <span class="settings-row-label">Context window</span>
                  <input
                    class="settings-text-input"
                    inputMode="numeric"
                    placeholder="1048576 (leave blank to infer from the id)"
                    value={contextWindow}
                    onInput={(e) =>
                      setContextWindow((e.currentTarget as HTMLInputElement).value)
                    }
                  />
                </div>
                <div class="settings-row">
                  <span class="settings-row-label" />
                  {/* A sentence, not a row of fields, so it takes the prose
                      modifier: the base class is a flex row whose 0.75rem gap is
                      the field separator (`.claude/rules/frontend.md`). */}
                  <span class="list-row-details list-row-details-prose">
                    Tokens. Left blank, the engine guesses from the model id. It has
                    no rule for OpenRouter, xAI, Gemini, or local models, so they are
                    treated as 200k however large they really are.
                  </span>
                </div>
                <div class="settings-row">
                  <span class="settings-row-label">Reads images</span>
                  <label class="toggle-switch">
                    <input
                      type="checkbox"
                      checked={vision}
                      aria-label="The new model reads images"
                      onChange={(e) => setVision((e.currentTarget as HTMLInputElement).checked)}
                    />
                    <span class="toggle-slider" />
                  </label>
                </div>
                <div class="settings-row">
                  <span class="settings-row-label" />
                  <div class="settings-row-options">
                    <button class="action-btn" onClick={() => setAdding(false)}>
                      Cancel
                    </button>
                    <button
                      class="action-btn action-btn-confirm"
                      disabled={!id.trim() || !label.trim()}
                      onClick={() => void add()}
                    >
                      Add
                    </button>
                  </div>
                </div>
              </div>
            ) : (
              <ListRowAddCard label="Add Model" onClick={() => setAdding(true)} />
            )}
          </>
        )}
      </LoadingFade>
    </div>
  );
}
