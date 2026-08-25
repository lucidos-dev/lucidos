import { useRef, useState } from 'preact/hooks';
import { useAnchoredPosition } from '../../hooks/useAnchoredPopover';
import { useHidePanelWebviewWhile } from '../../hooks/useHidePanelWebviewWhile';
import { Overlay } from './Overlay';
import { dropdownPanelStyle } from './Dropdown';
import { ModelSelectionPicker } from './ModelSelectionPicker';
import { useModelSelection, type ModelSelectionPatch } from '../../hooks/useModelSelection';
import type { ModelChoice, TierChoice } from '../../store/modelSelection';
import { focusIfNeeded } from '../../utils/dom';

/** A form field over a *model selection*: a dropdown-shaped trigger reading the
 *  whole pair, opening the same two-step picker the prompt bar mounts.
 *
 *  It wears `.dropdown-trigger` and opens a `.dropdown-menu` panel, so it sits
 *  in a Settings row or a `.form-group` like any other control. What it is NOT
 *  is a `Dropdown`: that one is a flat list of values, and a model selection is
 *  reached in two steps.
 *
 *  Bare on purpose, so a caller can put it in whatever field its surface uses:
 *  a Settings row, or the trigger form's `.form-group`. */
export function ModelSelectionField({
  label = 'Model',
  models,
  vocabulary,
  model,
  effort,
  disabled,
  onChange,
}: {
  /** The section label over the picker's model step. */
  label?: string;
  models: readonly ModelChoice[];
  vocabulary: readonly TierChoice[];
  model: string;
  effort: string | null;
  disabled?: boolean;
  onChange: (patch: ModelSelectionPatch) => void;
}) {
  const ref = useRef<HTMLDivElement>(null);
  const menuRef = useRef<HTMLDivElement>(null);
  const triggerRef = useRef<HTMLButtonElement>(null);
  // The anchor IS the open state, so `useAnchoredPosition` re-measures off its
  // own dep rather than a second flag. Same shape as `Dropdown`.
  const [anchor, setAnchor] = useState<HTMLElement | null>(null);
  const open = anchor !== null;
  const pos = useAnchoredPosition(anchor, menuRef);
  useHidePanelWebviewWhile(open);

  const selection = useModelSelection({ models, vocabulary, model, effort, onChange });

  function close() {
    setAnchor(null);
  }

  /** Close after a pick, putting focus back on the trigger: the panel that held
   *  it is about to unmount, which would drop a keyboard user out to `<body>`.
   *  Matches `Dropdown`'s `restoreFocusOnSelect`, and like it applies only to a
   *  pick. An outside click is the user going somewhere else, so taking focus
   *  back there would be stealing it. */
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
        <span class="dropdown-chevron">{open ? '▴' : '▾'}</span>
      </button>
      <Overlay
        open={open}
        onClose={close}
        anchor={ref.current}
        backdrop={false}
        // Portaled for the same reason `Dropdown` is. The panel carries
        // viewport coordinates, and a pane that is its own stacking context
        // would cap the z-index and clip the menu at its edge.
        portal
        panelClass="dropdown-menu"
        panelRef={menuRef}
        panelStyle={dropdownPanelStyle(anchor ? anchor.getBoundingClientRect().width : null, pos)}
      >
        <ModelSelectionPicker
          label={label}
          selection={selection}
          onPick={(encoded) => {
            selection.pick(encoded);
            closeAfterPick();
          }}
        />
      </Overlay>
    </div>
  );
}
