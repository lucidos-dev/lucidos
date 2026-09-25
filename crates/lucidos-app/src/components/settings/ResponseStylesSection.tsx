import { useEffect, useState } from 'preact/hooks';
import { responseStyles, responseStylesVersion, showConfirm } from '../../store/store';
import {
  currentResponseStyle,
  currentTechnicalLiteracy,
  setResponseStyle,
  setTechnicalLiteracy,
  TECHNICAL_LITERACY_LEVELS,
  TECHNICAL_LITERACY_NOT_SET,
  type TechnicalLiteracy,
} from '../../store/actions/preferences';
import { loadResponseStyles, saveStyleDocument } from '../../store/actions/responseStyles';
import { useVersionedRefresh } from '../../hooks/useVersionedRefresh';
import { useServerBackedField } from '../../hooks/useServerBackedField';
import { useDelayedLoading } from '../../hooks/useDelayedLoading';
import { Dropdown, DropdownSkeleton, type DropdownOption } from '../shared/Dropdown';
import { LoadingFade } from '../shared/LoadingFade';
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
 *  An edited shipped style says so, since its description is still the
 *  shipped one.
 *
 *  A pure function, so the row a user reads is testable without a DOM. */
export function responseStyleOptions(library: readonly ResponseStyle[]): DropdownOption[] {
  return library.map((style) => ({
    value: style.id,
    label: style.source === 'overridden' ? `${style.label} (edited)` : style.label,
    description: style.description,
  }));
}

/** Mirrors `card_label` / `card_line` in `core/technical_literacy.rs`, so
 *  Settings reads exactly like the first-run card. Pinned by the mirror test. */
const LITERACY_COPY: Record<TechnicalLiteracy, { label: string; description: string }> = {
  'non-technical': { label: 'Keep it plain', description: 'Everyday words, no jargon.' },
  technical: { label: 'Technical', description: 'Technical terms are fine.' },
  developer: { label: 'I write software', description: 'Talk to me like a developer.' },
};

/** The *technical literacy* picker's options, least technical first, after
 *  "Not set". A pure function, so the rows are testable without a DOM. */
export function technicalLiteracyOptions(): DropdownOption[] {
  return [
    {
      value: TECHNICAL_LITERACY_NOT_SET,
      label: 'Not set',
      description: 'Nothing is added. Answers keep their usual wording.',
    },
    ...TECHNICAL_LITERACY_LEVELS.map((level) => ({ value: level, ...LITERACY_COPY[level] })),
  ];
}

function onLiteracyPicked(value: string) {
  const level = TECHNICAL_LITERACY_LEVELS.find((l) => l === value) ?? null;
  void setTechnicalLiteracy(level);
}

/** Settings → Models → Response style: how answers come back. Two independent
 *  picks: the *style* (the shape of an answer) and the *technical literacy*.
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
  const showStyleSkeleton = useDelayedLoading(loadable);

  const library = loadable.status === 'loaded' ? loadable.data : [];
  // The style the editor is open on. Re-read from the library each render
  // rather than captured on Edit, so a landed save repaints the form.
  const editingStyle = editing === null ? undefined : library.find((s) => s.id === editing);
  const editorOpen = adding || editingStyle !== undefined;

  // Reloads whenever a peer device or the agent rewrites the document. The
  // version counter is the subscription (ADR 0118). The mount load is the
  // effect below.
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
            How answers come back, in two independent parts. <strong>Style</strong> sets
            the shape of an answer: how much comes back, and what it is for.{' '}
            <strong>How technical</strong> sets the words: plain
            language, or technical terms with no explanation. Any style works with any
            level, so a technical user who wants only the outcome picks their level and
            a short style.
          </p>
          <p>Both apply to chat and to every trigger, from your next message onward.</p>
          <p>
            <strong>Standard</strong> adds nothing at all, so answers come back the way
            they always have. Pick it to turn the whole setting off.
          </p>
          <p>
            <strong>Concise</strong>, <strong>Minimal</strong> and{' '}
            <strong>Learning</strong> ship with Lucidos, and you can edit any of them.
            Minimal gives the outcome, not the process. Learning explains the why as it
            goes. An edited style keeps a Reset, which brings the original wording back.
            Add your own with the card at the bottom.
          </p>
          <p>
            Whatever a style asks for, Lucidos still keeps every warning, every caveat
            that changes the answer, and every step you have to take. You cannot write
            that rule away, and asking "why" or "how" still gets a real explanation.
          </p>
          <p>
            Coding-agent sessions follow <strong>How technical</strong> from their next
            start. The style does not reach them: they have their own instructions.
          </p>
          <p>
            The setup guide asks how technical you are. You can change it here any time.
          </p>
        </Explainer>
      </div>

      {/* Above Style, so the style picker keeps its editor list right under it.
          It reads a preference rather than the library, so it never waits on
          the library load. */}
      <div class="settings-row" data-search-anchor="models:technical-literacy">
        <span class="settings-row-label">How technical</span>
        <Dropdown
          options={technicalLiteracyOptions()}
          value={currentTechnicalLiteracy() ?? TECHNICAL_LITERACY_NOT_SET}
          onChange={onLiteracyPicked}
        />
      </div>

      {loadable.status === 'failed' && (
        <LoadableError noun="response styles" error={loadable.error} />
      )}

      <div class="settings-row" data-search-anchor="models:response-style-picker">
        <span class="settings-row-label">Style</span>
        {/* Withheld until the library lands. Before that the trigger could only
            show the raw id and open an empty menu. */}
        <LoadingFade class="dropdown-slot" showSkeleton={showStyleSkeleton} skeleton={<DropdownSkeleton w="6rem" />}>
          {loadable.status === 'loaded' ? (
            <Dropdown
              options={responseStyleOptions(library)}
              value={selected}
              onChange={(id) => void setResponseStyle(id)}
            />
          ) : null}
        </LoadingFade>
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
        <div class="title">
          {style.label}
          {style.source === 'overridden' && <span class="style-row-edited">Edited</span>}
        </div>
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

/** Shown in the empty instruction field. Several lines, so it reads as a
 *  sample of what to write rather than as text already filled in. */
const INSTRUCTION_EXAMPLE = [
  'For example:',
  '- Start with the answer. Skip the introduction and the summary at the end.',
  '- Keep it to three bullet points or fewer.',
  '- Write for a busy manager who is not technical.',
].join('\n');

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
        // Re-derived against the CURRENT library. The render-time id above is
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
      {/* What a style is for. Above the fields rather than under them, so the
          reader meets it first. Each field then says what goes in it. */}
      <p class="style-editor-intro">
        A style tells Lucidos how to write its answers. While it is picked, Lucidos
        reads your instructions before every reply.
      </p>
      <label class="style-editor-field">
        <span class="style-editor-field-label">Name</span>
        <span class="style-editor-field-hint">What the style is called in the Style list.</span>
        <input
          class="settings-text-input"
          placeholder="For example: Board report"
          value={label}
          onInput={(e) => setLabel((e.currentTarget as HTMLInputElement).value)}
        />
      </label>
      {/* A plain textarea, never `AutoTextarea`. A field that grows to its own
          content keeps no height to scroll in, so a long instruction ran off
          the pane with no way to reach the end. */}
      <label class="style-editor-field style-editor-field-grow">
        <span class="style-editor-field-label">Instructions</span>
        <span class="style-editor-field-hint">
          How answers should read, in your own words. One rule per line works well.
        </span>
        <textarea
          class="style-editor-instruction"
          value={instruction}
          onInput={(e) => setInstruction((e.currentTarget as HTMLTextAreaElement).value)}
          placeholder={INSTRUCTION_EXAMPLE}
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
