import { useRef, useState } from 'preact/hooks';
import { useAnchoredPosition } from '../../hooks/useAnchoredPopover';
import { useHidePanelWebviewWhile } from '../../hooks/useHidePanelWebviewWhile';
import { Overlay } from './Overlay';
import { DropdownChevron, dropdownPanelStyle } from './dropdownPresentation';
import { ModelSelectionPicker, type PickerLink } from './ModelSelectionPicker';
import { useModelSelection, type ModelSelectionPatch } from '../../hooks/useModelSelection';
import type { ModelChoice, TierChoice } from '../../store/modelSelection';
import { focusIfNeeded } from '../../utils/dom';

export function ModelSelectionField({
  label = 'Model',
  models,
  vocabulary,
  model,
  effort,
  provider,
  effortFor,
  unsetEffortLabel,
  defaultEffortLabel,
  disabled,
  link,
  onChange,
}: {
  label?: string;
  models: readonly ModelChoice[];
  vocabulary: readonly TierChoice[];
  model: string;
  effort: string | null;
  provider?: string | null;
  effortFor?: (model: string) => string | null;
  unsetEffortLabel?: string;
  defaultEffortLabel?: (model: string) => string;
  disabled?: boolean;
  link?: PickerLink;
  onChange: (patch: ModelSelectionPatch) => void;
}) {
  const ref = useRef<HTMLDivElement>(null);
  const menuRef = useRef<HTMLDivElement>(null);
  const triggerRef = useRef<HTMLButtonElement>(null);
  const [anchor, setAnchor] = useState<HTMLElement | null>(null);
  const open = anchor !== null;
  const pos = useAnchoredPosition(anchor, menuRef);
  useHidePanelWebviewWhile(open);

  const selection = useModelSelection({
    models, vocabulary, model, effort, effortFor, unsetEffortLabel, defaultEffortLabel, onChange,
    providerFor: (m) => (m === model ? provider ?? null : null),
  });

  function close() {
    setAnchor(null);
  }

  function closeAfterPick() {
    close();
    focusIfNeeded(triggerRef.current);
  }

  return (
    <div class="dropdown model-selection-field" ref={ref}>
      <button
        ref={triggerRef}
        type="button"
        class="dropdown-trigger"
        disabled={disabled}
        aria-haspopup="listbox"
        aria-expanded={open}
        onClick={() => {
          if (disabled) return;
          setAnchor(open ? null : ref.current);
        }}
      >
        <span class="model-selection-value">{selection.label}</span>
        <DropdownChevron open={open} />
      </button>
      <Overlay
        open={open}
        onClose={close}
        anchor={ref.current}
        backdrop={false}
        portal
        panelClass="surface-box dropdown-menu"
        panelRef={menuRef}
        panelStyle={dropdownPanelStyle(anchor ? anchor.getBoundingClientRect().width : null, pos)}
      >
        <ModelSelectionPicker
          label={label}
          selection={selection}
          link={link && { option: link.option, onChoose: () => { close(); link.onChoose(); } }}
          onPick={(encoded, pickedProvider) => {
            selection.pick(encoded, pickedProvider);
            closeAfterPick();
          }}
        />
      </Overlay>
    </div>
  );
}
