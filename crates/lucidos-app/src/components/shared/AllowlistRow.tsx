import { SkBlock } from './Skeleton';

/** Pass no props inside a skeleton provider to render a placeholder row. */
export function AllowlistRow({ pattern, placeholder, onInput, onDelete, detail }: {
  pattern?: string;
  placeholder?: string;
  onInput?: (value: string) => void;
  onDelete?: () => void;
  detail?: string;
}) {
  return (
    <div class="allowlist-row">
      <SkBlock w="100%" h="2.25rem" round>
        {/* autocorrect/autocapitalize are deliberately NOT declared here: the
            global stamp (utils/noAutofill.ts) already turns both off for every
            field, and declaring autocorrect="off" in JSX would actually turn it
            ON — Preact assigns it as a DOM property, and `autocorrect`'s IDL
            attribute is a boolean, so the non-empty string "off" coerces to
            true. The stamp's setAttribute writes the literal keyword instead. */}
        <input
          class="allowlist-row-input"
          type="text"
          spellcheck={false}
          autocomplete="off"
          placeholder={placeholder}
          value={pattern}
          readOnly={!onInput}
          aria-label={!onInput ? pattern : undefined}
          onInput={(e) => onInput?.((e.target as HTMLInputElement).value)}
        />
      </SkBlock>
      {detail && <span>{detail}</span>}
      {(onDelete || !pattern) && <SkBlock w="3.75rem" h="2.25rem" round>
        <button
          type="button"
          class="action-btn action-btn-danger"
          aria-label={`Delete pattern ${pattern || '(empty)'}`}
          onClick={() => onDelete?.()}
        >
          Delete
        </button>
      </SkBlock>}
    </div>
  );
}
