import type { ComponentChildren } from 'preact';
import { credentials, preferences } from '../../store/store';
import {
  savePreference,
  saveModelSelection,
  storedBackgroundSelection,
} from '../../store/actions/preferences';
import { LUCIDOS_TIER_VOCABULARY } from '../../store/actions/models';
import type { ModelSelectionPatch } from '../../hooks/useModelSelection';
import { ModelSelectionRow } from './ModelSelectionRow';
import { backgroundModelChoices, backgroundRowCaveat } from './backgroundModels';
import type { BackgroundRows } from './useBackgroundModels';
import {
  judgmentModelChoices,
  judgmentPickWrites,
  judgmentSelectedEffort,
  judgmentSelectedModel,
  judgmentSelectionCaveat,
  offeredEndpoints,
  pickedEndpoint,
  storedJudgmentValue,
  systemOneConfigured,
  systemOneSwitchedOff,
  JUDGMENT_MODEL_KEY,
  JUDGMENT_PREFERENCE_KEY,
  JUDGMENT_REASONING_KEY,
  type JudgmentSite,
} from './judgmentBackend';

/**
 * One judgment site's *model selection*, with the System One rows among the
 * models.
 *
 * Shared by the two surfaces that own one, so the read, the writes and the
 * rule for offering System One rows cannot drift apart. Each site's row lives
 * beside the feature it changes rather than beside the key, which is the split
 * `model_command_judge` and `model_query_classification` already have.
 *
 * Every decision it makes is in `judgmentBackend.ts` and unit-tested there.
 * What lives here is the store read and the pair of writes.
 */
export function JudgmentModelRow(props: {
  site: JudgmentSite;
  label: string;
  anchor: string;
  /** What the engine resolves for the site's chat model, before any System
   *  One row. */
  background: BackgroundRows;
  /** Indent under the rows this one qualifies. */
  nested?: boolean;
  explainer?: ComponentChildren;
  /** A reason from the surrounding feature, such as the guard being off. */
  disabled?: boolean;
}) {
  // Both signals the row is derived from, read during render so it tracks
  // them. It waits on credentials too, because which System One rows are
  // offered depends on stored keys.
  const prefs = preferences.value;
  const creds = credentials.value;

  const picked = pickedEndpoint(storedJudgmentValue(prefs, props.site));
  const modelKey = JUDGMENT_MODEL_KEY[props.site];
  const reasoningKey = JUDGMENT_REASONING_KEY[props.site];

  // Offered only once BOTH reads have landed. An unloaded credential list reads
  // as no key. So an ungated rule drops a configured row out of the picker for
  // as long as the load takes.
  const loaded = prefs.status === 'loaded' && creds.status === 'loaded';
  const resolved = props.background.row(modelKey);
  const stored = storedBackgroundSelection(modelKey, reasoningKey);
  const chatModel = stored.model ?? resolved?.model ?? '';
  const models = judgmentModelChoices(
    backgroundModelChoices(resolved?.recommended ?? [], chatModel),
    loaded
      ? offeredEndpoints({
          picked,
          configured: (provider) => systemOneConfigured(provider, creds, prefs),
          switchedOff: (provider) => systemOneSwitchedOff(prefs, provider),
        })
      : [],
  );
  const pickedSwitchedOff = !!picked && systemOneSwitchedOff(prefs, picked.provider);
  const caveat = judgmentSelectionCaveat(picked, pickedSwitchedOff)
    ?? (picked ? null : backgroundRowCaveat(resolved, stored.model ? null : props.background.error));

  /** The judgment key goes first, because it is the one deciding which backend
   *  runs. Neither write can strand the other: `savePreference` never rejects,
   *  so the `await` only sequences them. */
  async function apply(patch: ModelSelectionPatch): Promise<void> {
    const writes = judgmentPickWrites(patch);
    await savePreference(JUDGMENT_PREFERENCE_KEY[props.site], writes.judgment);
    if (writes.selection) await saveModelSelection(modelKey, reasoningKey, writes.selection);
  }

  return (
    <ModelSelectionRow
      label={props.label}
      anchor={props.anchor}
      nested={props.nested}
      explainer={props.explainer}
      detail={caveat}
      models={models}
      vocabulary={LUCIDOS_TIER_VOCABULARY}
      model={judgmentSelectedModel(picked, chatModel)}
      effort={judgmentSelectedEffort(picked, stored.effort ?? resolved?.effort ?? null)}
      disabled={props.disabled}
      onChange={(patch) => void apply(patch)}
    />
  );
}
