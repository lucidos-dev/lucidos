import { assertArray } from './_validate';

export interface SelectOption {
  value: string;
  label: string;
  disabled?: boolean;
}

export interface SelectCreateOptions {
  options: SelectOption[];
  value?: string;
  placeholder?: string;
  disabled?: boolean;
  /** Extra class added to the root element. */
  className?: string;
  onChange?: (value: string, option: SelectOption | undefined) => void;
}

export interface SelectInstance {
  /** Root <div> — insert into the DOM. */
  readonly element: HTMLElement;
  getValue(): string | undefined;
  setValue(value: string | undefined): void;
  setOptions(options: SelectOption[]): void;
  setDisabled(disabled: boolean): void;
  open(): void;
  close(): void;
  /** Remove listeners and detach the element. */
  destroy(): void;
}

let _idSeq = 0;
function nextId(): string {
  _idSeq += 1;
  return `lucidos-select-${_idSeq}`;
}

function createSelect(opts: SelectCreateOptions): SelectInstance {
  if (!opts || typeof opts !== 'object') {
    throw new TypeError('Select.create requires an options object');
  }
  assertArray('options.options', opts.options);

  let options: SelectOption[] = opts.options.slice();
  let value: string | undefined = opts.value;
  let placeholder: string | undefined = opts.placeholder;
  let disabled = !!opts.disabled;
  const onChange = opts.onChange;

  let isOpen = false;
  let focusedIndex = -1;
  let typeBuffer = '';
  let typeBufferTimer: number | undefined;

  const id = nextId();
  const listboxId = `${id}-listbox`;

  const root = document.createElement('div');
  root.className = 'lucidos-select';
  if (opts.className) {
    for (const cls of opts.className.split(/\s+/)) if (cls) root.classList.add(cls);
  }
  root.dataset.state = 'closed';

  const trigger = document.createElement('button');
  trigger.type = 'button';
  trigger.className = 'lucidos-select-trigger';
  trigger.id = id;
  trigger.setAttribute('aria-haspopup', 'listbox');
  trigger.setAttribute('aria-expanded', 'false');
  trigger.setAttribute('aria-controls', listboxId);
  trigger.disabled = disabled;

  const labelEl = document.createElement('span');
  labelEl.className = 'lucidos-select-label';

  const chevron = document.createElement('span');
  chevron.className = 'lucidos-select-chevron';
  chevron.setAttribute('aria-hidden', 'true');
  chevron.textContent = '▾';

  trigger.appendChild(labelEl);
  trigger.appendChild(chevron);

  const menu = document.createElement('div');
  menu.className = 'lucidos-select-menu';
  menu.id = listboxId;
  menu.setAttribute('role', 'listbox');

  root.appendChild(trigger);
  root.appendChild(menu);

  function selectedOption(): SelectOption | undefined {
    return options.find((o) => o.value === value);
  }

  function updateLabel(): void {
    const sel = selectedOption();
    if (sel) {
      labelEl.textContent = sel.label;
      labelEl.classList.remove('lucidos-select-placeholder');
    } else {
      labelEl.textContent = placeholder ?? '';
      labelEl.classList.add('lucidos-select-placeholder');
    }
  }

  function renderOptions(): void {
    menu.innerHTML = '';
    options.forEach((o, i) => {
      const item = document.createElement('div');
      item.className = 'lucidos-select-option';
      item.setAttribute('role', 'option');
      item.id = `${id}-opt-${i}`;
      item.dataset.value = o.value;
      item.dataset.index = String(i);
      item.textContent = o.label;
      if (o.disabled) {
        item.classList.add('disabled');
        item.setAttribute('aria-disabled', 'true');
      }
      const isActive = o.value === value;
      item.setAttribute('aria-selected', isActive ? 'true' : 'false');
      if (isActive) item.classList.add('active');
      menu.appendChild(item);
    });
    syncFocusVisuals();
  }

  function syncFocusVisuals(): void {
    const items = menu.querySelectorAll<HTMLElement>('.lucidos-select-option');
    items.forEach((el, i) => el.classList.toggle('focused', i === focusedIndex));
    if (focusedIndex >= 0 && focusedIndex < options.length) {
      trigger.setAttribute('aria-activedescendant', `${id}-opt-${focusedIndex}`);
      const focused = items[focusedIndex];
      if (focused) scrollIntoMenu(focused);
    } else {
      trigger.removeAttribute('aria-activedescendant');
    }
  }

  function scrollIntoMenu(el: HTMLElement): void {
    const menuRect = menu.getBoundingClientRect();
    const elRect = el.getBoundingClientRect();
    if (elRect.top < menuRect.top) {
      menu.scrollTop -= menuRect.top - elRect.top;
    } else if (elRect.bottom > menuRect.bottom) {
      menu.scrollTop += elRect.bottom - menuRect.bottom;
    }
  }

  function firstEnabled(): number {
    for (let i = 0; i < options.length; i += 1) if (!options[i].disabled) return i;
    return -1;
  }

  function lastEnabled(): number {
    for (let i = options.length - 1; i >= 0; i -= 1) if (!options[i].disabled) return i;
    return -1;
  }

  function moveFocus(direction: 1 | -1): void {
    if (options.length === 0) return;
    let next = focusedIndex < 0 ? (direction === 1 ? -1 : options.length) : focusedIndex;
    for (let step = 0; step < options.length; step += 1) {
      next = (next + direction + options.length) % options.length;
      if (!options[next].disabled) {
        focusedIndex = next;
        syncFocusVisuals();
        return;
      }
    }
  }

  function setFocusTo(index: number): void {
    if (index < 0 || index >= options.length) return;
    if (options[index].disabled) return;
    focusedIndex = index;
    syncFocusVisuals();
  }

  function commitFocused(): void {
    if (focusedIndex < 0 || focusedIndex >= options.length) return;
    const o = options[focusedIndex];
    if (o.disabled) return;
    selectByValue(o.value);
    setOpen(false);
  }

  function selectByValue(v: string): void {
    if (v === value) return;
    value = v;
    updateLabel();
    renderOptions();
    if (onChange) onChange(v, options.find((o) => o.value === v));
  }

  function clearTypeBuffer(): void {
    typeBuffer = '';
    if (typeBufferTimer !== undefined) {
      clearTimeout(typeBufferTimer);
      typeBufferTimer = undefined;
    }
  }

  function handleType(ch: string): void {
    typeBuffer += ch.toLowerCase();
    if (typeBufferTimer !== undefined) clearTimeout(typeBufferTimer);
    typeBufferTimer = window.setTimeout(() => {
      typeBuffer = '';
      typeBufferTimer = undefined;
    }, 500);

    const start = focusedIndex < 0 ? 0 : focusedIndex;
    const total = options.length;
    if (total === 0) return;
    const tryMatch = (prefix: string): number => {
      const fromCurrent = typeBuffer.length === 1 ? 1 : 0;
      for (let step = fromCurrent; step <= total; step += 1) {
        const i = (start + step) % total;
        const o = options[i];
        if (o.disabled) continue;
        if (o.label.toLowerCase().startsWith(prefix)) return i;
      }
      return -1;
    };
    let match = tryMatch(typeBuffer);
    if (match < 0 && typeBuffer.length > 1) {
      typeBuffer = ch.toLowerCase();
      match = tryMatch(typeBuffer);
    }
    if (match >= 0) {
      focusedIndex = match;
      if (!isOpen) setOpen(true);
      else syncFocusVisuals();
    }
  }

  function setOpen(next: boolean): void {
    if (next === isOpen) return;
    if (next && disabled) return;
    isOpen = next;
    root.dataset.state = next ? 'open' : 'closed';
    trigger.setAttribute('aria-expanded', next ? 'true' : 'false');
    chevron.textContent = next ? '▴' : '▾';
    if (next) {
      const sel = selectedOption();
      const idx = sel ? options.indexOf(sel) : -1;
      focusedIndex = idx >= 0 && !options[idx].disabled ? idx : firstEnabled();
      syncFocusVisuals();
      document.addEventListener('mousedown', onDocumentMouseDown, true);
    } else {
      focusedIndex = -1;
      clearTypeBuffer();
      syncFocusVisuals();
      document.removeEventListener('mousedown', onDocumentMouseDown, true);
    }
  }

  function onTriggerClick(): void {
    if (disabled) return;
    setOpen(!isOpen);
  }

  function onTriggerKeyDown(e: KeyboardEvent): void {
    if (disabled) return;
    switch (e.key) {
      case 'ArrowDown':
        e.preventDefault();
        if (!isOpen) setOpen(true);
        else moveFocus(1);
        break;
      case 'ArrowUp':
        e.preventDefault();
        if (!isOpen) {
          setOpen(true);
          const last = lastEnabled();
          if (last >= 0) {
            focusedIndex = last;
            syncFocusVisuals();
          }
        } else {
          moveFocus(-1);
        }
        break;
      case 'Home':
        if (isOpen) {
          e.preventDefault();
          setFocusTo(firstEnabled());
        }
        break;
      case 'End':
        if (isOpen) {
          e.preventDefault();
          setFocusTo(lastEnabled());
        }
        break;
      case 'Enter':
      case ' ':
        e.preventDefault();
        if (!isOpen) setOpen(true);
        else commitFocused();
        break;
      case 'Escape':
        if (isOpen) {
          e.preventDefault();
          e.stopPropagation();
          setOpen(false);
        }
        break;
      case 'Tab':
        if (isOpen) setOpen(false);
        break;
      default:
        if (e.key.length === 1 && !e.ctrlKey && !e.metaKey && !e.altKey) {
          handleType(e.key);
        }
    }
  }

  function onMenuMouseDown(e: MouseEvent): void {
    // Keep focus on the trigger so the click that follows fires before any blur logic.
    e.preventDefault();
  }

  function onMenuClick(e: MouseEvent): void {
    const target = (e.target as Element | null)?.closest('.lucidos-select-option') as HTMLElement | null;
    if (!target || !menu.contains(target)) return;
    if (target.classList.contains('disabled')) return;
    const v = target.dataset.value;
    if (v === undefined) return;
    selectByValue(v);
    setOpen(false);
    trigger.focus();
  }

  function onDocumentMouseDown(e: MouseEvent): void {
    if (!root.contains(e.target as Node)) setOpen(false);
  }

  trigger.addEventListener('click', onTriggerClick);
  trigger.addEventListener('keydown', onTriggerKeyDown);
  menu.addEventListener('mousedown', onMenuMouseDown);
  menu.addEventListener('click', onMenuClick);

  updateLabel();
  renderOptions();

  return {
    element: root,
    getValue(): string | undefined {
      return value;
    },
    setValue(v: string | undefined): void {
      if (v === value) return;
      value = v;
      updateLabel();
      renderOptions();
    },
    setOptions(next: SelectOption[]): void {
      assertArray('options', next);
      options = next.slice();
      if (focusedIndex >= options.length) focusedIndex = -1;
      updateLabel();
      renderOptions();
    },
    setDisabled(d: boolean): void {
      disabled = !!d;
      trigger.disabled = disabled;
      if (disabled && isOpen) setOpen(false);
    },
    open(): void {
      setOpen(true);
    },
    close(): void {
      setOpen(false);
    },
    destroy(): void {
      setOpen(false);
      trigger.removeEventListener('click', onTriggerClick);
      trigger.removeEventListener('keydown', onTriggerKeyDown);
      menu.removeEventListener('mousedown', onMenuMouseDown);
      menu.removeEventListener('click', onMenuClick);
      document.removeEventListener('mousedown', onDocumentMouseDown, true);
      clearTypeBuffer();
      root.remove();
    },
  };
}

export const Select = {
  create: createSelect,
};

export function enhanceSelects(root: ParentNode = document): SelectInstance[] {
  const created: SelectInstance[] = [];
  const nodes = root.querySelectorAll<HTMLSelectElement>('select.lucidos-select');
  nodes.forEach((nativeSelect) => {
    if (nativeSelect.dataset.lucidosEnhanced === 'true') return;
    const items: SelectOption[] = Array.from(nativeSelect.options).map((o) => ({
      value: o.value,
      label: o.label || o.text,
      disabled: o.disabled,
    }));
    const inst = createSelect({
      options: items,
      value: nativeSelect.value || undefined,
      disabled: nativeSelect.disabled,
      placeholder: nativeSelect.dataset.placeholder,
      onChange: (v) => {
        nativeSelect.value = v;
        nativeSelect.dispatchEvent(new Event('change', { bubbles: true }));
      },
    });
    nativeSelect.style.display = 'none';
    nativeSelect.setAttribute('aria-hidden', 'true');
    nativeSelect.tabIndex = -1;
    nativeSelect.dataset.lucidosEnhanced = 'true';
    nativeSelect.parentNode!.insertBefore(inst.element, nativeSelect.nextSibling);
    created.push(inst);
  });
  return created;
}
