import type { ComponentChildren, JSX, Ref } from 'preact';
import { SearchIcon } from './icons';

/** The one search box in the host: a soft pill with a search glyph.
 *
 *  `class` goes on the pill, for the host's layout. `inputClass` names the
 *  field itself. `children` render inside the pill after the input, for a
 *  control that acts on the query, such as Clear. A control that closes the
 *  whole search belongs outside the pill. */
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
