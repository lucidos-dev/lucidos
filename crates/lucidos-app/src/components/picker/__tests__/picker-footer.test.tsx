/**
 * The picker footer's contract: the two entry points are always reachable, one
 * form at a time, and a Restore that cannot start always says why.
 */

import { describe, it, expect, vi } from 'vitest';
import type { ComponentChildren, VNode } from 'preact';
import { pickerFooter, type FooterMode, type PickerFooterProps } from '../PickerFooter';
import type { WorkspaceStatus } from '../../../api/client/control';
import { restoreBlocker, EMPTY_RESTORE_DRAFT, type RestoreDraft } from '../workspaceForms';
import { EMPTY_REMOTE_GATEWAY_DRAFT, TRUST_NOTE } from '../../settings/remoteGatewayForm';

/** A closed form rolls away, so the walks below read it as absent. */
function isClosedDisclosure(v: VNode<Record<string, unknown>>): boolean {
  return typeof v.type === 'function' && v.type.name === 'Disclosure' && !v.props?.open;
}

/** Flatten a vnode tree, keeping the markers we assert on. Mirrors
 *  network-access-popover.test.tsx. */
function vnodeToText(node: ComponentChildren): string {
  if (node === null || node === undefined || typeof node === 'boolean') return '';
  if (typeof node === 'string' || typeof node === 'number') return String(node);
  if (Array.isArray(node)) return node.map(vnodeToText).join('');
  const v = node as VNode<Record<string, unknown> & { children?: ComponentChildren }>;
  // Component vnodes render as their own name (nothing here renders them), so a
  // test can assert on the shared components the footer delegates to.
  const tag =
    typeof v.type === 'string' ? v.type : typeof v.type === 'function' ? v.type.name : '';
  if (isClosedDisclosure(v)) return '';
  const attrs = ['class', 'data-picked', 'aria-expanded', 'accept', 'disabled']
    .filter((k) => v.props?.[k] !== undefined && v.props?.[k] !== false)
    .map((k) => ` ${k}="${String(v.props[k])}"`)
    .join('');
  const inner = vnodeToText(v.props?.children);
  return tag ? `<${tag}${attrs}>${inner}</${tag}>` : inner;
}

const NOOP = () => {};

/** The remote gateway form's props, empty and inert. */
const REMOTE_DEFAULTS = {
  remoteDraft: EMPTY_REMOTE_GATEWAY_DRAFT,
  onRemoteDraft: NOOP,
  onAddRemote: NOOP,
  onCancelRemote: NOOP,
  pairedRemote: null,
};

function ws(id: string, name = id): WorkspaceStatus {
  return { id, name, port: 5000, health: 'healthy', autostart: true };
}

function render(over: Partial<PickerFooterProps> = {}): string {
  const draft = over.draft ?? EMPTY_RESTORE_DRAFT;
  return vnodeToText(
    pickerFooter({
      mode: 'none',
      onMode: NOOP,
      busy: false,
      restoreRunning: false,
      name: '',
      onName: NOOP,
      onCreate: NOOP,
      onCancelCreate: NOOP,
      suggestions: ['personal', 'work'],
      onSuggestion: NOOP,
      createNote: null,
      draft,
      onDraft: NOOP,
      onPickFile: NOOP,
      onRestore: NOOP,
      onCancelRestore: NOOP,
      blocker: restoreBlocker(draft, []),
      fileNote: null,
      onDeleteColliding: NOOP,
      ...REMOTE_DEFAULTS,
      ...over,
    }),
  );
}

const MODES: FooterMode[] = ['none', 'create', 'restore', 'remote'];

describe('both entry points are always on screen', () => {
  it('renders New workspace AND Restore from backup in every mode', () => {
    // The first-run bug: with zero workspaces the create form REPLACED the
    // footer, so the only path to restore was cancelling a form the user never
    // opened, in exactly the state restore exists for.
    for (const mode of MODES) {
      const text = render({ mode });
      expect(text, `mode=${mode}`).toContain('+ New workspace');
      expect(text, `mode=${mode}`).toContain('Restore from backup');
      expect(text, `mode=${mode}`).toContain('Add a remote gateway');
    }
  });

  it('marks the open one expanded and nothing else', () => {
    expect(render({ mode: 'none' })).not.toContain('aria-expanded="true"');
    expect(render({ mode: 'create' }).match(/aria-expanded="true"/g)).toHaveLength(1);
    expect(render({ mode: 'restore' }).match(/aria-expanded="true"/g)).toHaveLength(1);
  });

  it('opens exactly one form at a time', () => {
    expect(render({ mode: 'none' })).not.toContain('ws-picker-create');
    expect(render({ mode: 'none' })).not.toContain('ws-picker-restore-form');
    expect(render({ mode: 'create' })).not.toContain('ws-picker-restore-form');
    expect(render({ mode: 'restore' })).not.toContain('<div class="ws-picker-create">');
  });

  it('disables the restore entry point while one is already running', () => {
    expect(render({ mode: 'none', restoreRunning: true })).toContain('disabled="true"');
  });
});

describe('the quick-fill chips', () => {
  it('offers them while the name field is empty', () => {
    const text = render({ mode: 'create' });
    expect(text).toContain('ws-picker-suggestions-label');
    expect(text).toContain('personal');
  });

  it('drops the whole row, label included, when there are none', () => {
    // The picker sends no chips once the user has a workspace. A bare "Try"
    // with nothing after it is what a `suggestions.length` gate prevents.
    const text = render({ mode: 'create', suggestions: [] });
    expect(text).not.toContain('ws-picker-suggestions');
    expect(text).toContain('ws-picker-input');
  });

  it('hides them once the user types a name', () => {
    expect(render({ mode: 'create', name: 'fresh' })).not.toContain('ws-picker-suggestions');
  });
});

describe('the restore form explains itself', () => {
  const ready: RestoreDraft = { file: new File(['x'], 'b.enc'), key: 'k', name: 'fresh' };

  it('says what is missing whenever Restore is disabled', () => {
    // Reported as "the button is disabled, is it because the PWA is running?".
    // Nothing on screen said "choose a file".
    const text = render({ mode: 'restore' });
    expect(text).toContain('Choose the .age or .enc backup file to restore.');
  });

  it('drops the hint and enables Restore once the draft is complete', () => {
    const text = render({ mode: 'restore', draft: ready });
    expect(text).not.toContain('ws-picker-note');
    expect(text).not.toContain('<button class="ws-picker-btn ws-picker-btn-confirm" disabled="true">');
  });

  it('labels its fields instead of relying on placeholders alone', () => {
    const text = render({ mode: 'restore' });
    for (const label of ['Backup file', 'Backup key', 'Workspace name']) {
      expect(text).toContain(label);
    }
  });

  it('never constrains the file input by extension', () => {
    // `.enc` has no registered UTI, so an accept filter greys out every file in
    // the iOS Files picker and there is no way to restore from a phone.
    expect(render({ mode: 'restore' })).not.toContain('accept=');
  });

  it('picks the file through the shared off-screen input, not a display:none one', () => {
    // `display: none` (a bare `hidden` attribute) drops the change event on iOS
    // in standalone PWA mode, so the chosen file never arrives and the form sits
    // there looking half-filled. HiddenFileInput keeps it in layout, and must
    // stay inside the label so the tap is one native gesture.
    const text = render({ mode: 'restore' });
    expect(text).toContain('<HiddenFileInput>');
    expect(text).toMatch(/<label class="ws-picker-restore-drop[^"]*"[^>]*><HiddenFileInput>/);
  });

  it('marks a chosen file as chosen', () => {
    expect(render({ mode: 'restore' })).toContain('data-picked="false"');
    const picked = render({ mode: 'restore', draft: ready });
    expect(picked).toContain('data-picked="true"');
    expect(picked).toContain('b.enc');
  });
});

describe('a collision offers the workspace holding the address', () => {
  const renamed = [ws('personal', 'personaal')];
  const colliding: RestoreDraft = { file: new File(['x'], 'b.enc'), key: 'k', name: 'personal' };

  it('names the visible workspace and offers to delete THAT one', () => {
    const text = render({
      mode: 'restore',
      draft: colliding,
      blocker: restoreBlocker(colliding, renamed),
    });
    expect(text).toContain('/personal/');
    expect(text).toContain('Delete “personaal”…');
    expect(text).not.toContain('“personal” already exists');
  });

  it('hands the delete back by id, for the row confirm to take over', () => {
    const onDeleteColliding = vi.fn();
    const node = pickerFooter({
      mode: 'restore',
      onMode: NOOP,
      busy: false,
      restoreRunning: false,
      name: '',
      onName: NOOP,
      onCreate: NOOP,
      onCancelCreate: NOOP,
      suggestions: [],
      onSuggestion: NOOP,
      createNote: null,
      draft: colliding,
      onDraft: NOOP,
      onPickFile: NOOP,
      onRestore: NOOP,
      onCancelRestore: NOOP,
      blocker: restoreBlocker(colliding, renamed),
      fileNote: null,
      onDeleteColliding,
      ...REMOTE_DEFAULTS,
    });
    const button = findButton(node, 'Delete');
    (button?.props as { onClick?: () => void }).onClick?.();
    expect(onDeleteColliding).toHaveBeenCalledWith('personal');
    // The restore action itself never deletes: removing a workspace drops its
    // database, so it stays behind the row's type-the-name confirmation.
    expect(onDeleteColliding).toHaveBeenCalledTimes(1);
  });
});

/** The form's submitting field: the one `<input>` carrying an Enter handler (the
 *  create name, or the restore workspace-name). */
function findSubmittingInput(node: ComponentChildren): VNode<Record<string, unknown>> | null {
  if (node === null || node === undefined || typeof node !== 'object') return null;
  if (Array.isArray(node)) {
    for (const child of node) {
      const hit = findSubmittingInput(child);
      if (hit) return hit;
    }
    return null;
  }
  const v = node as VNode<Record<string, unknown> & { children?: ComponentChildren }>;
  if (isClosedDisclosure(v)) return null;
  if (v.type === 'input' && typeof v.props?.onKeyDown === 'function') return v;
  return findSubmittingInput(v.props?.children);
}

/** First `<button>` in the tree whose text starts with `label`. */
function findButton(node: ComponentChildren, label: string): VNode<Record<string, unknown>> | null {
  if (node === null || node === undefined || typeof node !== 'object') return null;
  if (Array.isArray(node)) {
    for (const child of node) {
      const hit = findButton(child, label);
      if (hit) return hit;
    }
    return null;
  }
  const v = node as VNode<Record<string, unknown> & { children?: ComponentChildren }>;
  if (isClosedDisclosure(v)) return null;
  if (v.type === 'button' && vnodeToText(v.props?.children).trim().startsWith(label)) return v;
  return findButton(v.props?.children, label);
}

describe('Enter cannot do what the button refuses', () => {
  /** Build the footer, press Enter in the first text input, report whether the
   *  submit handler fired. */
  function pressEnter(over: Partial<PickerFooterProps>, onSubmit: () => void): boolean {
    let fired = false;
    const node = pickerFooter({
      mode: 'none',
      onMode: NOOP,
      busy: false,
      restoreRunning: false,
      name: '',
      onName: NOOP,
      onCreate: () => { fired = true; onSubmit(); },
      onCancelCreate: NOOP,
      suggestions: [],
      onSuggestion: NOOP,
      createNote: null,
      draft: EMPTY_RESTORE_DRAFT,
      onDraft: NOOP,
      onPickFile: NOOP,
      onRestore: () => { fired = true; onSubmit(); },
      onCancelRestore: NOOP,
      blocker: null,
      fileNote: null,
      onDeleteColliding: NOOP,
      ...REMOTE_DEFAULTS,
      ...over,
    });
    const input = findSubmittingInput(node);
    (input?.props as { onKeyDown?: (e: unknown) => void }).onKeyDown?.({ key: 'Enter' });
    return fired;
  }

  it('does not create a workspace whose name is already taken', () => {
    // Found in review: the Create button was disabled but Enter still submitted,
    // so the only thing catching a duplicate was the gateway's 409.
    const blocked = { message: 'You already have a workspace called “work”.', blocking: true };
    expect(pressEnter({ mode: 'create', name: 'work', createNote: blocked }, NOOP)).toBe(false);
    expect(pressEnter({ mode: 'create', name: '  ' }, NOOP)).toBe(false);
    expect(pressEnter({ mode: 'create', name: 'fresh', busy: true }, NOOP)).toBe(false);
    expect(pressEnter({ mode: 'create', name: 'fresh' }, NOOP)).toBe(true);
  });

  it('does not start a restore that is still blocked', () => {
    const ready: RestoreDraft = { file: new File(['x'], 'b.enc'), key: 'k', name: 'fresh' };
    const blocker = restoreBlocker(EMPTY_RESTORE_DRAFT, []);
    // The restore form's own name field is the third input, so drive the whole
    // form instead: blocked draft, then a complete one.
    expect(pressEnter({ mode: 'restore', draft: EMPTY_RESTORE_DRAFT, blocker }, NOOP)).toBe(false);
    expect(pressEnter({ mode: 'restore', draft: ready, blocker: null }, NOOP)).toBe(true);
  });
});

describe('the remote gateway form', () => {
  const ready = { address: 'https://cloud.example.com', code: '12345678', name: 'cloud' };

  it('holds Pair until every field is filled', () => {
    const empty = render({ mode: 'remote' });
    expect(empty).toMatch(/<button class="ws-picker-btn ws-picker-btn-confirm" disabled="true">Pair/);
    const full = render({ mode: 'remote', remoteDraft: ready });
    expect(full).toMatch(/<button class="ws-picker-btn ws-picker-btn-confirm">Pair/);
  });

  it('says early that plain http will not pair', () => {
    const text = render({ mode: 'remote', remoteDraft: { ...ready, address: 'http://cloud.example.com' } });
    expect(text).toContain('https only');
    expect(text).toMatch(/ws-picker-btn-confirm" disabled="true">Pair/);
  });

  /** A paired gateway's pages run as this gateway's own, so the form says
   *  so before anyone fills it in. */
  it('warns to add only a gateway you run', () => {
    expect(render({ mode: 'remote' })).toContain(TRUST_NOTE);
  });

  it('names the gateway it just paired with', () => {
    expect(render({ mode: 'remote', pairedRemote: 'cloud' })).toContain('Paired with cloud.');
  });

  it('does not send on Enter while a field is empty', () => {
    let sent = 0;
    const press = (remoteDraft: typeof ready) => {
      const node = pickerFooter({
        ...propsFor('remote'),
        remoteDraft,
        onAddRemote: () => { sent += 1; },
      });
      const input = findSubmittingInput(node);
      (input?.props as { onKeyDown?: (e: unknown) => void }).onKeyDown?.({ key: 'Enter' });
    };
    press({ ...ready, code: '' });
    expect(sent).toBe(0);
    press(ready);
    expect(sent).toBe(1);
  });
});

/** Every prop, inert, in `mode`. */
function propsFor(mode: FooterMode): PickerFooterProps {
  return {
    mode,
    onMode: NOOP,
    busy: false,
    restoreRunning: false,
    name: '',
    onName: NOOP,
    onCreate: NOOP,
    onCancelCreate: NOOP,
    suggestions: [],
    onSuggestion: NOOP,
    createNote: null,
    draft: EMPTY_RESTORE_DRAFT,
    onDraft: NOOP,
    onPickFile: NOOP,
    onRestore: NOOP,
    onCancelRestore: NOOP,
    blocker: null,
    fileNote: null,
    onDeleteColliding: NOOP,
    ...REMOTE_DEFAULTS,
  };
}
