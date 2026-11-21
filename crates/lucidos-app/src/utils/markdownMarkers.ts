/**
 * The markers the markdown engine writes into its output. Surfaces that read
 * rendered markdown import them from here, so they never pull the engine
 * (`marked`, DOMPurify) into their own chunk.
 */

/** Unguessable, per module load, so content cannot name a copy slot.
 *
 *  Content authors both halves of a copy forgery: a real block hidden where
 *  nobody reads it, and a decoy label carrying that block's slot. Every marker
 *  the renderer writes carries this nonce, so content has no slot to name.
 *  `markdownEngine.ts` states where each marker is resolved. */
export const COPY_ID_NONCE = ((): string => {
  const bytes = new Uint8Array(8);
  if (typeof crypto !== 'undefined' && typeof crypto.getRandomValues === 'function') {
    crypto.getRandomValues(bytes);
  } else {
    for (let i = 0; i < bytes.length; i++) bytes[i] = Math.floor(Math.random() * 256);
  }
  return Array.from(bytes, (b) => b.toString(16).padStart(2, '0')).join('');
})();

/** Marks a code block this renderer wrote, so its Copy button gets a payload. */
export const CODE_COPY_ATTR = 'data-code-copy';

/** Marks a GFM task-list checkbox this renderer wrote, carrying the same
 *  nonce as the copy markers above. It is the only `<input>` the sanitizer
 *  keeps: an authored one shows as text (`markdownEngine.ts`).
 *  `MarkdownDocument`'s checkbox-toggle wiring also keys on it when mapping a
 *  click back to a source line (`taskListToggle.ts`). */
export const TASK_CHECKBOX_ATTR = 'data-task-checkbox';

/** The task checkbox's whole markup. The sanitizer rebuilds every kept
 *  checkbox from it, so no authored attribute rides on one. */
export const taskCheckboxTag = (checked: boolean): string =>
  `<input ${TASK_CHECKBOX_ATTR}="${COPY_ID_NONCE}"${checked ? ' checked' : ''} disabled type="checkbox">`;

/** Marks an email the GFM autolinker found in bare text, so the engine
 *  can unwrap one sitting inside a path. Its token's `raw` is the bare address;
 *  an authored `<x@y>` or `[x](mailto:x)` keeps its brackets in `raw`. */
export const BARE_EMAIL_ATTR = 'data-bare-email';

/** Marks a *widget embed* this renderer wrote (ADR 0415), carrying the copy
 *  nonce, so authored HTML cannot forge a slot. The slot holds the embed's
 *  label until `useWidgetEmbedSlots` mounts its widget, which happens only in
 *  a reply's final text: a streaming reply shows the label. */
export const WIDGET_EMBED_SLOT_ATTR = 'data-widget-embed-slot';
/** The slot's embed as JSON, or absent when its syntax is broken. */
export const WIDGET_EMBED_ATTR = 'data-widget-embed';
/** Why the slot's embed syntax is broken, beside a missing embed. */
export const WIDGET_EMBED_ERROR_ATTR = 'data-widget-embed-error';
