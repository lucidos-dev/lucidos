import type { ComponentChildren, JSX, Ref } from 'preact';
import { SearchIcon } from './SearchIcon';

export function SearchField({
  class: pillClass,
  inputClass,
  inputRef,
  children,
  placeholder,
  ...input
}: Omit<JSX.InputHTMLAttributes<HTMLInputElement>, 'class' | 'className' | 'ref' | 'type'> & {
  class?: string;
  inputClass?: string;
  inputRef?: Ref<HTMLInputElement>;
  children?: ComponentChildren;
}) {
  return (
    <div class={pillClass ? `search-field ${pillClass}` : 'search-field'}>
      <span class="search-field-icon" aria-hidden="true"><SearchIcon /></span>
      <input
        aria-label={typeof placeholder === 'string' ? placeholder : undefined}
        {...input}
        ref={inputRef}
        type="text"
        class={inputClass ? `search-field-input ${inputClass}` : 'search-field-input'}
        placeholder={placeholder}
      />
      {children}
    </div>
  );
}
