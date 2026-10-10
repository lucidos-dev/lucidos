import { useState } from 'preact/hooks';
import type { Ref } from 'preact';
import { EyeIcon, EyeOffIcon } from './icons';

interface SecretInputProps {
  /** Forwarded to the inner <input>, so callers can read the value via a ref. */
  inputRef?: Ref<HTMLInputElement>;
  /** Initial value (uncontrolled). Used to pre-fill the current secret on edit. */
  defaultValue?: string;
  placeholder?: string;
  required?: boolean;
}

/** A masked secret field: shows dots by default, with an eye toggle to reveal
 *  the actual text. Uncontrolled — read the value through `inputRef`. */
export function SecretInput({ inputRef, defaultValue, placeholder, required }: SecretInputProps) {
  const [revealed, setRevealed] = useState(false);
  return (
    <div class="secret-input">
      <input
        ref={inputRef}
        type={revealed ? 'text' : 'password'}
        defaultValue={defaultValue}
        placeholder={placeholder}
        required={required}
        autocomplete="off"
        spellcheck={false}
      />
      <button
        type="button"
        class="secret-input-toggle"
        aria-label={revealed ? 'Hide secret' : 'Show secret'}
        data-tooltip={revealed ? 'Hide' : 'Show'}
        onClick={() => setRevealed((v) => !v)}
      >
        {revealed ? <EyeOffIcon /> : <EyeIcon />}
      </button>
    </div>
  );
}
