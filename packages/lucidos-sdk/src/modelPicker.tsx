import { render } from 'preact';
import { useRef, useLayoutEffect } from 'preact/hooks';
import { signal } from '@preact/signals';
import { ModelSelectionPicker } from '../../../crates/lucidos-app/src/components/shared/ModelSelectionPicker';
import { DropdownChevron, dropdownPanelStyle } from '../../../crates/lucidos-app/src/components/shared/dropdownPresentation';
import { createPortal } from 'preact/compat';
import { pushOverlay, removeOverlay, topPanelOverlay } from '../../../crates/lucidos-app/src/store/overlayStack';
import { useDismissOnOutside } from '../../../crates/lucidos-app/src/hooks/useAnchoredPopover';
import { installLocalOverlayEscape } from './_localOverlayEscape';
import { useAnchoredPosition } from '../../../crates/lucidos-app/src/hooks/useAnchoredPopover';
import { useModelSelection } from '../../../crates/lucidos-app/src/hooks/useModelSelection';
import { REASONING_LEVELS } from '../../../crates/lucidos-app/src/store/reasoningLevels';
import { type ModelChoice } from '../../../crates/lucidos-app/src/store/modelSelection';
import { request } from './request';
import { modelChoices, resolveSelection, type RegistryModel, type ModelPickerValue, type ModelPickerFilter } from './_modelRegistry';

export type { ModelPickerValue, ModelPickerFilter } from './_modelRegistry';

export interface ModelPickerOptions {
  value: ModelPickerValue;
  onChange: (value: ModelPickerValue) => void;
  filter?: ModelPickerFilter;
  disabled?: boolean;
  label?: string;
  onError?: (error: unknown) => void;
}
export interface ModelPickerInstance {
  element: HTMLDivElement;
  ready: Promise<void>;
  refresh(): Promise<void>;
  getValue(): ModelPickerValue;
  setValue(value: ModelPickerValue): void;
  setDisabled(disabled: boolean): void;
  open(): void;
  close(): void;
  destroy(): void;
}

let nextPickerId = 0;

function create(options: ModelPickerOptions): ModelPickerInstance {
  const element = document.createElement('div');
  const overlayId = `sdk-model-picker-${++nextPickerId}`;
  const offEscape = installLocalOverlayEscape();
  const value = signal({ ...options.value });
  const choices = signal<readonly ModelChoice[]>([]);
  const opened = signal(false);
  const disabled = signal(options.disabled ?? false);
  const loading = signal(true);
  const error = signal<unknown>(null);
  let destroyed = false;
  let sequence = 0;
  function assertAlive() { if (destroyed) throw new Error('ModelPicker is destroyed'); }

  function Field() {
    const anchor = useRef<HTMLDivElement>(null);
    const trigger = useRef<HTMLButtonElement>(null);
    const panel = useRef<HTMLDivElement>(null);
    const pos = useAnchoredPosition(opened.value ? trigger.current : null, panel);
    const close = () => { opened.value = false; };
    useLayoutEffect(() => {
      if (!opened.value) return;
      pushOverlay({ id: overlayId, hasPanel: true, dismiss: () => { close(); trigger.current?.focus(); } });
      return () => removeOverlay(overlayId);
    }, [opened.value]);
    useDismissOnOutside(opened.value, panel, anchor.current, close,
      () => topPanelOverlay()?.id === overlayId);
    const selection = useModelSelection({
      models: choices.value, vocabulary: REASONING_LEVELS,
      model: value.value.model, effort: value.value.reasoning_effort,
      providerFor: model => model === value.value.model ? value.value.provider : null,
      unsetEffortLabel: 'Default', defaultEffortLabel: () => 'Default',
      onChange: patch => {
        const resolved = resolveSelection(choices.value, {
          model: patch.model, reasoning_effort: patch.reasoningEffort,
          provider: patch.provider ?? selection.rows.find(row => row.value === patch.model)?.provider ?? null,
        });
        value.value = resolved;
        opened.value = false;
        trigger.current?.focus();
        options.onChange({ ...resolved });
      },
    });
    return <div class="dropdown model-selection-field" ref={anchor}>
      <button type="button" class="dropdown-trigger" ref={trigger}
        aria-label={options.label ?? 'Model'} aria-haspopup="listbox" aria-expanded={opened.value}
        disabled={disabled.value || loading.value || error.value !== null}
        onKeyDown={event => {
          if ((event.key === 'ArrowDown' || event.key === 'ArrowUp') && !disabled.value && !loading.value && !error.value) {
            event.preventDefault();
            opened.value = true;
          }
        }}
        onClick={() => { opened.value = !opened.value; }}>
        <span class="model-selection-value">{error.value ? 'Models unavailable' : selection.label}</span>
        <DropdownChevron open={opened.value} />
      </button>
      {opened.value && createPortal(
        <div class="surface-box dropdown-menu" ref={panel} data-overlay-panel={overlayId}
          onKeyDown={event => {
            if (event.key === 'Tab') {
              opened.value = false;
              // Restore DOM order before the browser performs native traversal.
              trigger.current?.focus();
            }
          }}
          style={dropdownPanelStyle(trigger.current?.getBoundingClientRect().width ?? null, pos)}>
          <ModelSelectionPicker label={options.label ?? 'Model'} selection={selection}
            disabled={disabled.value} onPick={(encoded, provider) => selection.pick(encoded, provider)} />
        </div>, document.body)}
    </div>;
  }
  render(<Field />, element);

  async function refresh() {
    assertAlive();
    const attempt = ++sequence;
    loading.value = true;
    opened.value = false;
    try {
      const registry = await request<{ models: RegistryModel[] }>('/models');
      assertAlive();
      if (attempt !== sequence) return;
      const next = modelChoices(registry.models, options.filter);
      const resolved = resolveSelection(next, value.value);
      choices.value = next;
      value.value = resolved;
      error.value = null;
    } catch (cause) {
      if (!destroyed && attempt === sequence) {
        error.value = cause;
        options.onError?.(cause);
      }
      throw cause;
    } finally {
      if (!destroyed && attempt === sequence) loading.value = false;
    }
  }
  const ready = refresh();
  // `ready` remains rejectable to callers while an optional onError-only user
  // does not create an unhandled rejection during initial loading.
  void ready.catch(() => {});
  return {
    element, ready, refresh,
    getValue: () => { assertAlive(); return { ...value.value }; },
    setValue: next => {
      assertAlive();
      value.value = loading.value ? { ...next } : resolveSelection(choices.value, next);
      opened.value = false;
    },
    setDisabled: next => { assertAlive(); disabled.value = next; if (next) opened.value = false; },
    open: () => { assertAlive(); if (!disabled.value && !loading.value && !error.value) opened.value = true; },
    close: () => { assertAlive(); opened.value = false; },
    destroy: () => {
      if (destroyed) return;
      destroyed = true;
      ++sequence;
      render(null, element);
      element.remove();
      offEscape();
    },
  };
}

export const ModelPicker = { create };
