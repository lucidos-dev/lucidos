import type { ComponentChildren } from 'preact';
import { configuredProviders, credentials, preferences } from '../../store/store';
import {
  savePreference,
  saveModelSelection,
  storedBackgroundSelection,
} from '../../store/actions/preferences';
import { LUCIDOS_TIER_VOCABULARY, PROVIDER_DEFAULT_EFFORT_LABEL } from '../../store/actions/models';
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
  sitePick,
  systemOneConfigured,
  systemOneSwitchedOff,
  JUDGMENT_MODEL_KEY,
  JUDGMENT_PREFERENCE_KEY,
  JUDGMENT_REASONING_KEY,
  type JudgmentSite,
} from './judgmentBackend';

export function JudgmentModelRow(props: {
  site: JudgmentSite;
  label: string;
  anchor: string;
  background: BackgroundRows;
  nested?: boolean;
  explainer?: ComponentChildren;
}) {
  const prefs = preferences.value;
  const creds = credentials.value;

  const picked = sitePick(props.site, prefs, configuredProviders.value);
  const modelKey = JUDGMENT_MODEL_KEY[props.site];
  const reasoningKey = JUDGMENT_REASONING_KEY[props.site];

  const loaded = prefs.status === 'loaded' && creds.status === 'loaded';
  const resolved = props.background.row(modelKey);
  const stored = storedBackgroundSelection(modelKey, reasoningKey);
  const chatModel = stored.model ?? resolved?.model ?? '';
  const models = judgmentModelChoices(
    backgroundModelChoices(resolved?.recommended ?? [], chatModel),
    loaded
      ? offeredEndpoints({
          picked,
          configured: (provider) => systemOneConfigured(provider, creds, prefs, configuredProviders.value),
          switchedOff: (provider) => systemOneSwitchedOff(prefs, provider),
        })
      : [],
  );
  const pickedSwitchedOff = !!picked && systemOneSwitchedOff(prefs, picked.provider);
  const caveat = judgmentSelectionCaveat(picked, pickedSwitchedOff)
    ?? (picked ? null : backgroundRowCaveat(resolved, stored.model ? null : props.background.error));

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
      unsetEffortLabel={PROVIDER_DEFAULT_EFFORT_LABEL}
      onChange={(patch) => void apply(patch)}
    />
  );
}
