import { useEffect, useState } from 'preact/hooks';
import { responseStyles, responseStylesVersion, showConfirm } from '../../store/store';
import { currentResponseStyle, setResponseStyle } from '../../store/actions/preferences';
import { loadResponseStyles, saveStyleDocument } from '../../store/actions/responseStyles';
import { useVersionedRefresh } from '../../hooks/useVersionedRefresh';
import { useServerBackedField } from '../../hooks/useServerBackedField';
import { Dropdown, type DropdownOption } from '../shared/Dropdown';
import { Explainer } from '../shared/Explainer';
import { LoadableError } from '../shared/LoadableError';
import { ListRowAddCard } from '../shared/ListRowAddCard';
import { Overlay } from '../shared/Overlay';
import { PROSE_TEXT_ATTRS } from '../../utils/noAutofill';
import {
  STANDARD_ID,
  MAX_INSTRUCTION_CHARS,
  describeStyleProblem,
  documentWithEdit,
  documentWithout,
  libraryIsFull,
  nextStyleId,
} from './responseStyle';
import type { ResponseStyle } from '../../api/types';

/** The picker's options: every style in the library, each with its own one-line
 *  description. Standard leads, because it is the default and the way back.
 *
 *  A pure function, so the row a user reads is testable without a DOM. */
export function responseStyleOptions(library: readonly ResponseStyle[]): DropdownOption[] {
  return library.map((style) => ({
    value: style.id,
    label: style.label,
    description: style.description,
  }));
}

/** Settings → Models → Response style: which *response style* answers come
 *  back in.
 *
 *  The picker and the editor are one component because they read one list.
 *  That list comes from the engine (`GET /api/v1/response-styles`), merged from
 *  what Lucidos ships and what this workspace saved. So the shipped
 *  instructions have a single home and cannot drift out of a copy kept here.
 */
export function ResponseStylesSection() {
  const loadable = responseStyles.value;
  const selected = currentResponseStyle();
  const [editing, setEditing] = useState<string | null>(null);
  const [adding, setAdding] = useState(false);

  const library = loadable.status === 'loaded' ? loadable.data : [];
  // The style the editor is open on. Re-read from the library each render
  // rather than captured on Edit, so a landed save repaints the form.
  const editingStyle = editing === null ? undefined : library.find((s) => s.id === editing);
  const editorOpen = adding || editingStyle !== undefined;

  // Loads on mount, and again whenever a peer device or the agent rewrites the
  // document. The version counter is the subscription (ADR 0118).
  //
  // Paused on the editor being OPEN, never on the raw `editing` id: an id the
  // library no longer answers draws nothing, and pausing on it would leave the
  // refresh switched off with no way back.
  useVersionedRefresh(responseStylesVersion.value, editorOpen, () => {
    void loadResponseStyles();
  });
  // In an effect, not the render body: `loadResponseStyles` writes the very
  // signal this component read above, and a setState during render trips preact
  // lint. Every other load on this page moved out for the same reason.
  useEffect(() => {
    if (responseStyles.value.status === 'not-loaded') void loadResponseStyles();
  }, []);

  // A selected id the library no longer holds. The engine falls back to
  // Standard, so the row says so rather than showing a blank trigger.
  const missing = library.length > 0 && !library.some((s) => s.id === selected);

  function closeEditor() {
    setEditing(null);
    setAdding(false);
  }

  return (
    <div class="settings-section">
      <div class="settings-section-title" data-search-anchor="models:response-style">
        Response style
        <Explainer title="Response style">
          <p>
            How much comes back in an answer. It applies to chat and to every trigger,
            from your next message onward.
          </p>
          <p>
            <strong>Standard</strong> adds nothing at all, so answers come back the way
            they always have. Pick it to turn the whole setting off.
          </p>
          <p>
            <strong>Concise</strong> and <strong>Minimal</strong> ship with Lucidos, and
            you can edit either. An edited one keeps a Reset, which brings the original
            wording back. Add your own with the card at the bottom.
          </p>
          <p>
            Whatever a style asks for, Lucidos still keeps every warning, every caveat
            that changes the answer, and every step you have to take. You cannot write
            that rule away, and asking "why" or "how" still gets a real explanation.
          </p>
          <p>Coding-agent sessions are unaffected: they have their own instructions.</p>
        </Explainer>
      </div>

      {loadable.status === 'failed' && (
        <LoadableError noun="response styles" error={loadable.error} />
      )}

      <div class="settings-row" data-search-anchor="models:response-style-picker">
        <span class="settings-row-label">Style</span>
        <Dropdown
          options={responseStyleOptions(library)}
          value={selected}
          onChange={(id) => void setResponseStyle(id)}
        />
      </div>
      {missing && (
        <div class="settings-row-note">
          The style saved here no longer exists, so answers come back as Standard. Pick
          one to fix it.
        </div>
      )}

      {loadable.status === 'loaded' && (
        <div class="list-rows">
          {library
            .filter((style) => style.editable)
            .map((style) => (
              <StyleRow
                key={style.id}
                style={style}
                onEdit={() => setEditing(style.id)}
              />
            ))}
          {!libraryIsFull(library) && (
            <ListRowAddCard label="Add Style" onClick={() => setAdding(true)} />
          )}
        </div>
      )}

      {editorOpen && (
        <StyleEditorModal
          key={editingStyle?.id ?? 'add'}
          style={editingStyle}
          library={library}
          onDone={closeEditor}
        />
      )}
    </div>
  );
}

/** One editable style at rest. Standard never reaches here: it draws no
 *  buttons, because the off switch has nothing to edit or delete. */
function StyleRow({
  style,
  onEdit,
}: {
  style: ResponseStyle;
  onEdit: () => void;
}) {
  // Reset and Delete are the same write, and which one it is follows from where
  // the row came from. Both drop the row's entry from the document: on a
  // shipped style that entry is the override, on a user style it is the style.
  const shipped = style.source !== 'user';
  async function drop() {
    const question = shipped
      ? `Reset "${style.label}" to the wording Lucidos ships?`
      : `Delete the "${style.label}" style?`;
    if (!(await showConfirm(question, shipped ? 'Reset' : 'Delete'))) return;
    await saveStyleDocument((current) => documentWithout(current, style.id));
  }

  return (
    <div class="list-row">
      <div class="list-row-info">
        <div class="title">{style.label}</div>
        <div class="list-row-details list-row-details-prose">{style.description}</div>
      </div>
      <div class="list-row-actions">
        <button class="action-btn" onClick={onEdit}>
          Edit
        </button>
        {style.source === 'overridden' && (
          <button class="action-btn" onClick={() => void drop()}>
            Reset
          </button>
        )}
        {style.source === 'user' && (
          <button class="action-btn action-btn-danger" onClick={() => void drop()}>
            Delete
          </button>
        )}
      </div>
    </div>
  );
}

/** The edit form, shared by Edit and Add. `style` absent means Add.
 *
 *  A modal rather than a row that opens in place. An instruction runs to a
 *  thousand characters, and the settings pane on a phone is too narrow to read
 *  one in. The panel takes the width the viewport allows, and hands the
 *  leftover height to the instruction. That field scrolls inside its own box
 *  (`styles/settings/response-style-editor.css`).
 *
 *  The instruction is server-backed. An untouched form repaints when a frame
 *  lands, and a touched one keeps the draft. Seeding a `useState` from the row
 *  is the bug that replaces (`.claude/rules/frontend.md`). */
function StyleEditorModal({
  style,
  library,
  onDone,
}: {
  style?: ResponseStyle;
  library: readonly ResponseStyle[];
  onDone: () => void;
}) {
  const [label, setLabel] = useServerBackedField(style?.label ?? '');
  const [instruction, setInstruction] = useServerBackedField(style?.instruction ?? '');
  const [saving, setSaving] = useState(false);

  // A shipped style keeps its id whatever the label says. Deriving one from the
  // label would save an edit of Concise as a new style, leaving the original
  // in place and unedited.
  const taken = style
    ? library.filter((s) => s.id !== style.id).map((s) => s.id)
    : library.map((s) => s.id);
  const id = style ? style.id : nextStyleId(label, taken);
  // The bound is on the DOCUMENT, so it is measured on what this save would
  // store. Overriding an untouched shipped style adds a row, which a full
  // library must refuse before Save rather than after the engine does.
  const nextSize = documentWithEdit(library, { id, label, instruction }).length;
  const problem = describeStyleProblem(
    { id, label, instruction },
    [...taken, STANDARD_ID],
    nextSize,
  );
  const used = [...instruction.trim()].length;
  const title = style ? `Edit "${style.label}"` : 'Add a style';

  async function save() {
    if (problem) return;
    setSaving(true);
    try {
      const ok = await saveStyleDocument((current) => {
        // Re-derived against the CURRENT library. The render-time id below is
        // only a validity preview: computed from a stale list it could collide
        // with a style another device added, which `documentWithEdit` would
        // then overwrite in place.
        const freshId = style ? style.id : nextStyleId(label, current.map((s) => s.id));
        return documentWithEdit(current, { id: freshId, label, instruction });
      });
      // Only a landed write closes the form. A refused one keeps the paragraph
      // on screen beside the toast saying what was wrong with it.
      if (ok) onDone();
    } finally {
      setSaving(false);
    }
  }

  return (
    <Overlay
      open
      onClose={onDone}
      overlayClass="style-editor-overlay"
      panelClass="style-editor-modal"
      panelRole="dialog"
      ariaModal
      dataRole="response-style-editor"
      panelProps={{ 'aria-label': title }}
    >
      <h2 class="style-editor-title">{title}</h2>
      {/* The one place the instruction's job is explained. Above the fields
          rather than under them, so the reader meets it first. */}
      <p class="style-editor-intro">
        What this should do to an answer, written as instructions to Lucidos. It is
        added to every reply, word for word.
      </p>
      <label class="style-editor-field">
        <span class="style-editor-field-label">Name</span>
        <input
          class="settings-text-input"
          placeholder="Board report"
          value={label}
          onInput={(e) => setLabel((e.currentTarget as HTMLInputElement).value)}
        />
      </label>
      {/* A plain textarea, never `AutoTextarea`. A field that grows to its own
          content keeps no height to scroll in, so a long instruction ran off
          the pane with no way to reach the end. */}
      <label class="style-editor-field style-editor-field-grow">
        <span class="style-editor-field-label">Instruction</span>
        <textarea
          class="style-editor-instruction"
          value={instruction}
          onInput={(e) => setInstruction((e.currentTarget as HTMLTextAreaElement).value)}
          placeholder="- Lead with the answer. No preamble, no closing recap."
          {...PROSE_TEXT_ATTRS}
        />
      </label>
      {problem && <p class="style-editor-problem">{problem}</p>}
      <div class="style-editor-foot">
        <span class="style-editor-count">
          {used} of {MAX_INSTRUCTION_CHARS} characters
        </span>
        <div class="style-editor-actions">
          <button class="action-btn" onClick={onDone}>
            Cancel
          </button>
          <button
            class="action-btn action-btn-confirm"
            disabled={problem !== null || saving}
            onClick={() => void save()}
          >
            Save
          </button>
        </div>
      </div>
    </Overlay>
  );
}
