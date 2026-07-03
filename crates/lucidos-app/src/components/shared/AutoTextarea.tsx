import { useRef, useEffect } from 'preact/hooks';
import { autoResizeTextarea } from '../../utils/dom';

interface Props {
  value: string;
  onInput: (value: string) => void;
  placeholder?: string;
  required?: boolean;
}

/**
 * Auto-expanding textarea for modal form fields.
 * - Grows/shrinks to fit content (min 1 row)
 * - Inside a <form>: Enter submits, Shift+Enter inserts a newline
 * - Outside a <form>: Enter inserts a newline (default browser behavior)
 */
export function AutoTextarea({ value, onInput, placeholder, required }: Props) {
  const ref = useRef<HTMLTextAreaElement>(null);

  useEffect(() => autoResizeTextarea(ref.current), [value]);

  function handleInput(e: Event) {
    onInput((e.target as HTMLTextAreaElement).value);
  }

  function handleKeyDown(e: KeyboardEvent) {
    if (e.key === 'Enter' && !e.shiftKey) {
      const form = ref.current?.form;
      if (form) {
        e.preventDefault();
        form.requestSubmit();
      }
    }
  }

  return (
    <textarea
      ref={ref}
      class="auto-textarea"
      value={value}
      onInput={handleInput}
      onKeyDown={handleKeyDown}
      placeholder={placeholder}
      required={required}
      rows={1}
    />
  );
}
