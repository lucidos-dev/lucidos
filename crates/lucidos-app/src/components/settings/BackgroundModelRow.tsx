import { LUCIDOS_TIER_VOCABULARY } from '../../store/actions/models';
import {
  saveModelSelection,
  storedBackgroundSelection,
  type BackgroundModelKey,
  type BackgroundReasoningKey,
} from '../../store/actions/preferences';
import { ModelSelectionRow } from './ModelSelectionRow';
import { backgroundModelChoices, backgroundRowCaveat } from './backgroundModels';
import type { BackgroundRows } from './useBackgroundModels';

/** One background task's *model selection* row. While unset it shows the
 *  model the engine resolved, and its picker lists the task's recommended
 *  models first. */
export function BackgroundModelRow({
  label,
  anchor,
  nested,
  modelKey,
  reasoningKey,
  background,
}: {
  label: string;
  anchor?: string;
  nested?: boolean;
  modelKey: BackgroundModelKey;
  reasoningKey: BackgroundReasoningKey;
  background: BackgroundRows;
}) {
  const resolved = background.row(modelKey);
  const stored = storedBackgroundSelection(modelKey, reasoningKey);
  const model = stored.model ?? resolved?.model ?? '';
  // A stored pick shows without the engine's answer, so a failed read is
  // news only while the row is unset.
  const caveat = backgroundRowCaveat(resolved, stored.model ? null : background.error);
  return (
    <ModelSelectionRow
      label={label}
      anchor={anchor}
      nested={nested}
      detail={caveat && <span class="error-text">{caveat}</span>}
      models={backgroundModelChoices(
        resolved?.recommended ?? [],
        model,
        resolved?.needs_vision ?? false,
      )}
      vocabulary={LUCIDOS_TIER_VOCABULARY}
      model={model}
      effort={stored.effort ?? resolved?.effort ?? null}
      onChange={(p) => void saveModelSelection(modelKey, reasoningKey, p)}
    />
  );
}
