import { useState } from 'preact/hooks';
import type { ComponentChildren, Ref } from 'preact';
import { SecretInput } from '../shared/SecretInput';
import { isSecretReference } from './credentialSecret';

interface SecretFieldProps {
  /** The field's label: a `<label>`, or a `FieldLabel` with its explainer. */
  label: ComponentChildren;
  inputRef: Ref<HTMLInputElement>;
  /** The stored value, which may already be a reference. */
  initialValue: string;
  placeholder: string;
  required?: boolean;
}

/** A secret form field that holds either the value itself or a *secret
 *  reference*, picked by the "From 1Password" switch beside its label.
 *
 *  The input remounts on a switch, so a pasted value never stays in the field
 *  as a reference, or the reverse. The engine resolves a reference on save
 *  and names the reason when it cannot. */
export function SecretField({ label, inputRef, initialValue, placeholder, required }: SecretFieldProps) {
  const storedReference = isSecretReference(initialValue);
  const [fromOnePassword, setFromOnePassword] = useState(storedReference);
  const initialForSource = storedReference === fromOnePassword ? initialValue : '';
  return (
    <div class="form-group">
      <div class="secret-field-label-row">
        {label}
        <label class="secret-field-source">
          From 1Password
          <span class="toggle-switch">
            <input
              type="checkbox"
              checked={fromOnePassword}
              onChange={(e) => setFromOnePassword((e.currentTarget as HTMLInputElement).checked)}
            />
            <span class="toggle-slider" />
          </span>
        </label>
      </div>
      {fromOnePassword ? (
        <>
          <input
            key="reference"
            ref={inputRef}
            type="text"
            defaultValue={initialForSource}
            placeholder="op://vault/item/field"
            required={required}
            spellcheck={false}
          />
          <p class="secret-field-hint">Read from 1Password each time it is used. Never stored here.</p>
        </>
      ) : (
        <SecretInput
          key="value"
          inputRef={inputRef}
          defaultValue={initialForSource}
          placeholder={placeholder}
          required={required}
        />
      )}
    </div>
  );
}
