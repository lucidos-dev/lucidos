import type { ComponentChildren } from 'preact';
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
  explainer,
  detail,
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
  /** An `<Explainer>` beside the label, for a row whose choices differ in
   *  something the model names cannot say. */
  explainer?: ComponentChildren;
  /** A sentence under the label, for something true of the CURRENT selection
   *  rather than of the choices. An explainer is folded away behind a tap, so
   *  it cannot carry a caveat the user has to see. */
  detail?: ComponentChildren;
  models: readonly ModelChoice[];
  vocabulary: readonly TierChoice[];
  model: string;
  effort: string | null;
  disabled?: boolean;
  onChange: (patch: ModelSelectionPatch) => void;
}) {
  return (
    <div class={`settings-row${nested ? ' settings-row-child' : ''}`} data-search-anchor={anchor}>
      <span class="settings-row-label">
        {label}
        {explainer}
        {detail && <span class="list-row-details list-row-details-prose">{detail}</span>}
      </span>
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
