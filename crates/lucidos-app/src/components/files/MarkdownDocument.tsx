import { useLayoutEffect, useMemo, useRef } from 'preact/hooks';
import { renderMarkdown } from '../../utils/renderMarkdown';
import { splitFrontmatter, type Frontmatter } from '../../utils/frontmatter';
import { COPY_ID_NONCE, TASK_CHECKBOX_ATTR } from '../../utils/markedConfig';
import { countTaskListItems } from '../../utils/taskListToggle';

/** A markdown file as a document: its frontmatter as a properties card, then
 *  the rendered body. The raw text stays in the editor and the Source view.
 *
 *  `editable` lets the reader flip a GFM task-list checkbox in place. Left
 *  unset, every checkbox stays `disabled`, which is `marked`'s own default
 *  for a task-list item. `onToggleCheckbox` takes the clicked checkbox's
 *  0-based index among all task items in the body, matching
 *  `utils/taskListToggle.ts`'s counting order (fenced code blocks skipped,
 *  top to bottom). It resolves `true` on a successful save, `false`
 *  otherwise, so the checkbox can revert a failed toggle. If the rendered
 *  checkbox count ever disagrees with that count, every checkbox stays
 *  disabled instead: see the fail-closed comment inside the wiring effect. */
export function MarkdownDocument({ content, onClick, editable = false, onToggleCheckbox }: {
  content: string;
  onClick?: (e: MouseEvent) => void;
  editable?: boolean;
  onToggleCheckbox?: (taskIndex: number) => Promise<boolean>;
}) {
  const { frontmatter, html } = useMemo(() => {
    const split = splitFrontmatter(content);
    return { frontmatter: split.frontmatter, html: renderMarkdown(split.body) };
  }, [content]);
  const bodyRef = useRef<HTMLDivElement>(null);
  // The latest callback, read inside the click handler rather than closed
  // over by the effect below. A caller re-rendering with a fresh function
  // identity (no reason to memoize a per-file callback) must NOT re-run the
  // wiring effect: that would re-enable a box mid-save, letting a second
  // click race the first toggle's re-read-then-write.
  const onToggleRef = useRef(onToggleCheckbox);
  onToggleRef.current = onToggleCheckbox;

  // Un-disables each task-list checkbox and wires its click, in an effect
  // rather than the renderer: `renderMarkdown` is shared with chat, which
  // never wants an interactive checkbox, so this stays local to the one
  // surface that does. The deps are what changes the checkbox set or its
  // clickability; `onToggleRef` above is why `onToggleCheckbox` is not one.
  //
  // A layout effect, not a plain effect: a save confirming replaces this
  // whole subtree with a freshly rendered, freshly `disabled` checkbox
  // (`marked`'s default). A plain effect un-disables it one paint late,
  // which read as the checkbox blinking off and back on. The layout
  // effect runs before paint, so the swap never shows.
  useLayoutEffect(() => {
    const body = bodyRef.current;
    // `onToggleCheckbox` here is the prop snapshot, not `onToggleRef`:
    // `onToggleRef` deliberately never re-triggers this effect on its own,
    // so a late-arriving handler still needs the snapshot to un-disable.
    if (!body || !editable || !onToggleCheckbox) return;
    // Scoped to checkboxes `marked` generated for a real `- [ ]` line (see
    // `TASK_CHECKBOX_ATTR`), never a raw `<input type="checkbox">` the
    // content authored: that one has no matching line for
    // `taskListToggle.ts` to find, and would misalign every later index.
    const boxes = Array.from(body.querySelectorAll<HTMLInputElement>(`input[type="checkbox"][${TASK_CHECKBOX_ATTR}="${COPY_ID_NONCE}"]`));
    // Fails closed: `taskListToggle.ts` has no real block parser. A
    // construct it does not model (an indented code block, say) can make it
    // find a different count than what actually rendered. Leaving every box
    // disabled is safer than risking a write that targets the wrong line.
    if (boxes.length !== countTaskListItems(content)) return;
    const cleanups = boxes.map((box, index) => {
      box.removeAttribute(TASK_CHECKBOX_ATTR);
      box.disabled = false;
      const handleClick = () => {
        const toggle = onToggleRef.current;
        if (!toggle) return;
        // The browser already flipped `box.checked` as the click's default
        // action, before this listener runs: free optimistic UI. A failed
        // save reverts that flip, so the reader never sees a checked state
        // the file on disk does not have.
        const checkedBeforeClick = !box.checked;
        box.disabled = true;
        void toggle(index).then((ok) => {
          if (!ok) box.checked = checkedBeforeClick;
          box.disabled = false;
        });
      };
      box.addEventListener('click', handleClick);
      return () => box.removeEventListener('click', handleClick);
    });
    return () => cleanups.forEach((cleanup) => cleanup());
    // `!!onToggleCheckbox`, not the callback itself: presence is what gates
    // clickability, and a fresh identity for the same presence must not
    // re-run this (see the comment on `onToggleRef` above).
  }, [html, editable, content, !!onToggleCheckbox]);

  return (
    <>
      {frontmatter && <FrontmatterCard frontmatter={frontmatter} />}
      <div
        ref={bodyRef}
        class="response-content markdown-content"
        onClick={onClick ? (e) => onClick(e as unknown as MouseEvent) : undefined}
        dangerouslySetInnerHTML={{ __html: html }}
      />
    </>
  );
}

export function FrontmatterCard({ frontmatter }: { frontmatter: Frontmatter }) {
  if (frontmatter.kind === 'raw') {
    return <pre class="frontmatter-card frontmatter-card-raw" aria-label="Properties">{frontmatter.text}</pre>;
  }
  return (
    <dl class="frontmatter-card" aria-label="Properties">
      {frontmatter.fields.map(([key, value], row) => (
        <div class="frontmatter-row" key={row}>
          <dt>{key}</dt>
          {Array.isArray(value)
            ? <dd class="frontmatter-chips">{value.map((item, i) => <span class="frontmatter-chip" key={i}>{item}</span>)}</dd>
            : <dd>{value}</dd>}
        </div>
      ))}
    </dl>
  );
}
