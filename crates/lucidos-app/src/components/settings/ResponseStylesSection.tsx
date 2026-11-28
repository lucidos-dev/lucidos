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
import { SurfaceHead } from '../shared/Surface';
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

export function responseStyleOptions(library: readonly ResponseStyle[]): DropdownOption[] {
  return library.map((style) => ({
    value: style.id,
    label: style.source === 'overridden' ? `${style.label} (edited)` : style.label,
    description: style.description,
  }));
}

const LITERACY_COPY: Record<TechnicalLiteracy, { label: string; description: string }> = {
  'non-technical': { label: 'Keep it plain', description: 'Everyday words, no jargon.' },
  technical: { label: 'Technical', description: 'Technical terms are fine.' },
  developer: { label: 'I write software', description: 'Talk to me like a developer.' },
};

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

export function ResponseStylesSection() {
  const loadable = responseStyles.value;
  const selected = currentResponseStyle();
  const [editing, setEditing] = useState<string | null>(null);
  const [adding, setAdding] = useState(false);
  const showStyleSkeleton = useDelayedLoading(loadable);

  const library = loadable.status === 'loaded' ? loadable.data : [];
  const editingStyle = editing === null ? undefined : library.find((s) => s.id === editing);
  const editorOpen = adding || editingStyle !== undefined;

  useVersionedRefresh(responseStylesVersion.value, editorOpen, () => {
    void loadResponseStyles();
  });
  useEffect(() => {
    if (responseStyles.value.status === 'not-loaded') void loadResponseStyles();
  }, []);

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

function StyleRow({
  style,
  onEdit,
}: {
  style: ResponseStyle;
  onEdit: () => void;
}) {
  const shipped = style.source !== 'user';
  async function drop() {
    const question = shipped
      ? `Reset "${style.label}" to the wording Lucidos ships?`
      : `Delete the "${style.label}" style?`;
    if (!(await showConfirm(question, shipped ? 'Reset' : 'Delete', { variant: 'danger' }))) return;
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

const INSTRUCTION_EXAMPLE = [
  'For example:',
  '- Start with the answer. Skip the introduction and the summary at the end.',
  '- Keep it to three bullet points or fewer.',
  '- Write for a busy manager who is not technical.',
].join('\n');

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

  const taken = style
    ? library.filter((s) => s.id !== style.id).map((s) => s.id)
    : library.map((s) => s.id);
  const id = style ? style.id : nextStyleId(label, taken);
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
        const freshId = style ? style.id : nextStyleId(label, current.map((s) => s.id));
        return documentWithEdit(current, { id: freshId, label, instruction });
      });
      if (ok) onDone();
    } finally {
      setSaving(false);
    }
  }

  return (
    <Overlay
      open
      onClose={onDone}
      panelClass="surface surface-raised style-editor-modal"
      panelRole="dialog"
      ariaModal
      dataRole="response-style-editor"
      panelProps={{ 'aria-label': title }}
    >
      <SurfaceHead title={title} onClose={onDone} closeLabel={`Close ${title}`} />
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
          <button class="action-btn action-btn-secondary" onClick={onDone}>
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
