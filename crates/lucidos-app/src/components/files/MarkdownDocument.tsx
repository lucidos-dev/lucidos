import { useLayoutEffect, useMemo, useRef } from 'preact/hooks';
import { markdownEngine, renderMarkdown } from '../../utils/renderMarkdown';
import type { MarkdownDocumentLocation } from '../../utils/markdownImageSource';
import { splitFrontmatter, type Frontmatter } from '../../utils/frontmatter';
import { COPY_ID_NONCE, TASK_CHECKBOX_ATTR } from '../../utils/markdownMarkers';
import { countTaskListItems } from '../../utils/taskListToggle';

export function MarkdownDocument({ content, location, onClick, editable = false, onToggleCheckbox }: {
  content: string;
  location: MarkdownDocumentLocation;
  onClick?: (e: MouseEvent) => void;
  editable?: boolean;
  onToggleCheckbox?: (taskIndex: number) => Promise<boolean>;
}) {
  const locationKey = JSON.stringify(location);
  const markdown = markdownEngine.value;
  const { frontmatter, html } = useMemo(() => {
    const split = splitFrontmatter(content);
    return { frontmatter: split.frontmatter, html: renderMarkdown(split.body, { document: location }) };
  }, [content, locationKey, markdown]);
  const bodyRef = useRef<HTMLDivElement>(null);
  const onToggleRef = useRef(onToggleCheckbox);
  onToggleRef.current = onToggleCheckbox;

  useLayoutEffect(() => {
    const body = bodyRef.current;
    if (!body || !editable || !onToggleCheckbox) return;
    const boxes = Array.from(body.querySelectorAll<HTMLInputElement>(`input[type="checkbox"][${TASK_CHECKBOX_ATTR}="${COPY_ID_NONCE}"]`));
    if (boxes.length !== countTaskListItems(content)) return;
    const cleanups = boxes.map((box, index) => {
      box.removeAttribute(TASK_CHECKBOX_ATTR);
      box.disabled = false;
      const label = wrapInTaskLabel(box);
      const handleClick = () => {
        const toggle = onToggleRef.current;
        if (!toggle) return;
        const checkedBeforeClick = !box.checked;
        box.disabled = true;
        void toggle(index).then((ok) => {
          if (!ok) box.checked = checkedBeforeClick;
          box.disabled = false;
        });
      };
      box.addEventListener('click', handleClick);
      label.addEventListener('click', ignoreRepeatClicks);
      return () => {
        box.removeEventListener('click', handleClick);
        label.removeEventListener('click', ignoreRepeatClicks);
        label.replaceWith(...label.childNodes);
        box.setAttribute(TASK_CHECKBOX_ATTR, COPY_ID_NONCE);
        box.disabled = true;
      };
    });
    return () => cleanups.forEach((cleanup) => cleanup());
  }, [html, editable, content, !!onToggleCheckbox]);

  return (
    <>
      {frontmatter && <FrontmatterCard frontmatter={frontmatter} />}
      <div
        ref={bodyRef}
        class="response-content markdown-content authored-html"
        onClick={onClick ? (e) => onClick(e as unknown as MouseEvent) : undefined}
        dangerouslySetInnerHTML={{ __html: html }}
      />
    </>
  );
}

const TASK_LINE_INLINE = new Set([
  'A', 'ABBR', 'B', 'BR', 'CODE', 'DEL', 'EM', 'I', 'IMG', 'INPUT', 'INS',
  'KBD', 'MARK', 'S', 'SMALL', 'SPAN', 'STRONG', 'SUB', 'SUP', 'U',
]);

function wrapInTaskLabel(box: HTMLInputElement): HTMLLabelElement {
  const label = document.createElement('label');
  label.className = 'task-list-label';
  box.before(label);
  let node: ChildNode | null = box;
  while (node && !(node instanceof Element && !TASK_LINE_INLINE.has(node.tagName))) {
    const next: ChildNode | null = node.nextSibling;
    label.append(node);
    node = next;
  }
  return label;
}

function ignoreRepeatClicks(e: MouseEvent): void {
  if (e.detail > 1 && !(e.target instanceof HTMLInputElement)) e.preventDefault();
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
