import type { ComponentChildren } from 'preact';
import { ModelSelectionField } from '../shared/ModelSelectionField';
import type { PickerLink } from '../shared/ModelSelectionPicker';
import type { ModelSelectionPatch } from '../../hooks/useModelSelection';
import type { ModelChoice, TierChoice } from '../../store/modelSelection';

export function ModelSelectionRow({
  label,
  fieldLabel,
  anchor,
  nested,
  explainer,
  detail,
  models,
  vocabulary,
  model,
  effort,
  effortFor,
  unsetEffortLabel,
  defaultEffortLabel,
  disabled,
  link,
  onChange,
}: {
  label: string;
  fieldLabel?: string;
  anchor?: string;
  nested?: boolean;
  explainer?: ComponentChildren;
  detail?: ComponentChildren;
  models: readonly ModelChoice[];
  vocabulary: readonly TierChoice[];
  model: string;
  effort: string | null;
  effortFor?: (model: string) => string | null;
  unsetEffortLabel?: string;
  defaultEffortLabel?: (model: string) => string;
  disabled?: boolean;
  link?: PickerLink;
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
        label={fieldLabel ?? label}
        models={models}
        vocabulary={vocabulary}
        model={model}
        effort={effort}
        effortFor={effortFor}
        unsetEffortLabel={unsetEffortLabel}
        defaultEffortLabel={defaultEffortLabel}
        disabled={disabled}
        link={link}
        onChange={onChange}
      />
    </div>
  );
}
