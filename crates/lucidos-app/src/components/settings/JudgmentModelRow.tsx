import type { ComponentChildren } from 'preact';
import { credentials, preferences } from '../../store/store';
import {
  currentBackgroundModel,
  currentBackgroundReasoning,
  savePreference,
  saveModelSelection,
} from '../../store/actions/preferences';
import { LUCIDOS_TIER_VOCABULARY } from '../../store/actions/models';
import type { ModelChoice } from '../../store/modelSelection';
import type { ModelSelectionPatch } from '../../hooks/useModelSelection';
import { ModelSelectionRow } from './ModelSelectionRow';
import {
  jevRowOffered,
  judgmentModelChoices,
  judgmentPickWrites,
  judgmentSelectedEffort,
  judgmentSelectedModel,
  judgmentSelectionCaveat,
  storedJudgmentValue,
  typeSafeKeyStored,
  typeSafeSwitchedOff,
  wantsJev,
  JUDGMENT_MODEL_KEY,
  JUDGMENT_PREFERENCE_KEY,
  JUDGMENT_REASONING_KEY,
  type JudgmentSite,
} from './judgmentBackend';

/**
 * One classification's *model selection*, with TypeSafe (Jev) among the models.
 *
 * Shared by the two surfaces that own one, so the read, the writes and the rule
 * for offering Jev cannot drift apart. Each site's row lives beside the feature
 * it changes rather than beside the key, which is the split
 * `model_command_judge` and `model_query_classification` already have.
 *
 * Every decision it makes is in `judgmentBackend.ts` and unit-tested there.
 * What lives here is the store read and the pair of writes.
 */
export function JudgmentModelRow(props: {
  site: JudgmentSite;
  label: string;
  anchor: string;
  /** The chat models this site may run on, before Jev is considered. */
  models: readonly ModelChoice[];
  /** Indent under the rows this one qualifies. */
  nested?: boolean;
  explainer?: ComponentChildren;
  /** A reason from the surrounding feature, such as the guard being off. */
  disabled?: boolean;
}) {
  // Both signals the row is derived from, read during render so it tracks
  // them. It waits on credentials too, because whether Jev is offered at all
  // depends on a stored key.
  const prefs = preferences.value;
  const creds = credentials.value;

  const onJev = wantsJev(storedJudgmentValue(prefs, props.site));
  const modelKey = JUDGMENT_MODEL_KEY[props.site];
  const reasoningKey = JUDGMENT_REASONING_KEY[props.site];
  const typeSafeOff = typeSafeSwitchedOff(prefs);

  // Offered only once BOTH reads have landed. An unloaded credential list reads
  // as no key. So an ungated rule drops the Jev row out of a workspace that has
  // one, for as long as the load takes.
  const loaded = prefs.status === 'loaded' && creds.status === 'loaded';
  const models = judgmentModelChoices(
    props.models,
    loaded && jevRowOffered({ onJev, keyStored: typeSafeKeyStored(creds), typeSafeOff }),
  );

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
      detail={judgmentSelectionCaveat(onJev, typeSafeOff)}
      models={models}
      vocabulary={LUCIDOS_TIER_VOCABULARY}
      model={judgmentSelectedModel(onJev, currentBackgroundModel(modelKey))}
      effort={judgmentSelectedEffort(onJev, currentBackgroundReasoning(reasoningKey))}
      disabled={props.disabled}
      onChange={(patch) => void apply(patch)}
    />
  );
}
