import { ModelSelectionField } from '../shared/ModelSelectionField';
import type { ModelSelectionPatch } from '../../hooks/useModelSelection';
import type { ModelChoice, TierChoice } from '../../store/modelSelection';

/** The Settings row for a *model selection*: one label, one field.
 *
 *  ONE control, because a model selection is one thing. Four background
 *  purposes are four rows here, where a Model row plus a Reasoning row made
 *  eight. */
export function ModelSelectionRow({
  label,
  anchor,
  nested,
  models,
  vocabulary,
  model,
  effort,
  disabled,
  onChange,
}: {
  label: string;
  /** `data-search-anchor` for the row. */
  anchor?: string;
  /** The row sits under a parent control, so it indents. Layout only. */
  nested?: boolean;
  models: readonly ModelChoice[];
  vocabulary: readonly TierChoice[];
  model: string;
  effort: string | null;
  disabled?: boolean;
  onChange: (patch: ModelSelectionPatch) => void;
}) {
  return (
    <div class={`settings-row${nested ? ' settings-row-child' : ''}`} data-search-anchor={anchor}>
      <span class="settings-row-label">{label}</span>
      <ModelSelectionField
        label={label}
        models={models}
        vocabulary={vocabulary}
        model={model}
        effort={effort}
        disabled={disabled}
        onChange={onChange}
      />
    </div>
  );
}
