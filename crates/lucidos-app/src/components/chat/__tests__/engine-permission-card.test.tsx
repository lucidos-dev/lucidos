// @vitest-environment jsdom
import { afterEach, beforeEach, describe, expect, it, vi } from 'vitest';
// @ts-expect-error: Node APIs available at runtime via Vitest, no @types/node in project
import { readFileSync } from 'node:fs';
// @ts-expect-error: same
import { fileURLToPath } from 'node:url';
// @ts-expect-error: same
import { dirname, resolve } from 'node:path';
import { render } from 'preact';
import { act } from 'preact/test-utils';

const answerThreadQuestion = vi.fn((..._args: unknown[]) => new Promise<'sent'>(() => {}));
vi.mock('../../../store/actions/chat-claude-code', async (importOriginal) => ({
  ...(await importOriginal<typeof import('../../../store/actions/chat-claude-code')>()),
  answerThreadQuestion: (...args: unknown[]) => answerThreadQuestion(...args),
}));

import {
  CANCEL_LABEL,
  EnginePermissionBody,
  McpPermissionBody,
  enginePermissionButtons,
  permissionSlot,
} from '../PermissionCard';
import { pendingAnswers } from '../../../store/pendingDecisions';
import type { Action, Holder, Lifetime, PermissionChoice, PermissionRequest } from '../../../generated/thread-event-wire';
import type { QuestionOption } from '../../../store/thread-events';

const here = dirname(fileURLToPath(import.meta.url));
const MENU_RS = readFileSync(resolve(here, '../../../../../lucidos-engine/src/engine/permission/menu.rs'), 'utf8');

interface MenuRow { action: Action; thisThread: boolean; always: boolean }

const kebab = (pascal: string) => pascal.replace(/(?<!^)([A-Z])/g, '-$1').toLowerCase();

function menuRows(): MenuRow[] {
  const block = MENU_RS.slice(
    MENU_RS.indexOf('let (this_thread, always) = match action {'),
    MENU_RS.indexOf('Some(Menu {'),
  );
  const arm = /((?:Action::\w+\s*\|?\s*)+)=>\s*\((None|Some\(\w+\)),\s*(None|Some\(\w+\))\)/g;
  const rows: MenuRow[] = [];
  for (const [, names, thisThread, always] of block.matchAll(arm)) {
    for (const [, name] of names.matchAll(/Action::(\w+)/g)) {
      rows.push({ action: kebab(name) as Action, thisThread: thisThread !== 'None', always: always !== 'None' });
    }
  }
  return rows;
}

const asker: Holder = { kind: 'thread', thread_id: 'thread-1' };

function requestFor(row: MenuRow): { options: QuestionOption[]; request: PermissionRequest } {
  const grant = (lifetime: Lifetime, holder: Holder): PermissionChoice => ({ kind: 'grant', lifetime, holder, breadth: 'asked' });
  const entries: [QuestionOption, PermissionChoice][] = [
    [{ id: 'once', label: 'Allow once', description: 'Just this one time.' }, grant('once', asker)],
    ...(row.thisThread ? [[{ id: 'thread', label: 'Allow for this thread' }, grant('this_thread', asker)] as [QuestionOption, PermissionChoice]] : []),
    ...(row.always ? [[{ id: 'always', label: 'Always allow' }, grant('standing', { kind: 'workspace' })] as [QuestionOption, PermissionChoice]] : []),
    [{ id: 'refuse', label: "Don't allow" }, { kind: 'refuse' }],
  ];
  return {
    options: entries.map(([option]) => option),
    request: { action: row.action, choices: Object.fromEntries(entries.map(([option, choice]) => [option.id, choice])) },
  };
}

describe('the menu the card is read from', () => {
  it('answers the actions an agent asks for, and some of them stay once-only', () => {
    const rows = menuRows();
    expect(rows.length).toBeGreaterThan(10);
    expect(rows.some(row => !row.thisThread && !row.always)).toBe(true);
    expect(rows.some(row => row.thisThread && row.always)).toBe(true);
  });
});

describe('permissionSlot', () => {
  it('puts a refusal under Deny, a one-off grant under Allow once, and every lasting grant under Remember', () => {
    expect(permissionSlot({ kind: 'refuse' })).toBe('deny');
    expect(permissionSlot({ kind: 'refuse_standing' })).toBe('remember');
    expect(permissionSlot({ kind: 'grant', lifetime: 'once', holder: asker })).toBe('once');
    expect(permissionSlot({ kind: 'grant', lifetime: 'this_thread', holder: asker })).toBe('remember');
    expect(permissionSlot({ kind: 'grant', lifetime: 'until_plan_lands', holder: asker })).toBe('remember');
    expect(permissionSlot({ kind: 'grant', lifetime: 'standing', holder: { kind: 'workspace' } })).toBe('remember');
  });

  it('leaves every other choice an ordinary button, never a hidden one', () => {
    expect(permissionSlot({ kind: 'unpause' })).toBe('other');
    expect(permissionSlot({ kind: 'revoke' })).toBe('other');
    expect(permissionSlot(undefined)).toBe('other');
  });
});

describe.each(menuRows())('the card of $action', (row) => {
  const { options, request } = requestFor(row);
  const buttons = enginePermissionButtons(options, request, () => {});
  const remembered = buttons.filter(button => button.remembered);

  it('shows Deny first and Allow once second', () => {
    expect(buttons.slice(0, 2).map(button => button.choice)).toEqual(['deny', 'allow']);
  });

  it('puts exactly the lasting choices the menu allows in Remember', () => {
    expect(remembered.map(button => button.optionId)).toEqual([
      ...(row.thisThread ? ['thread'] : []),
      ...(row.always ? ['always'] : []),
    ]);
    for (const button of remembered) {
      const choice = request.choices[button.optionId];
      expect(choice.kind === 'grant' && choice.lifetime !== 'once').toBe(true);
    }
  });

  it('draws a Remember menu only for two or more lasting choices, and one lasting choice as its own button', () => {
    const host = document.createElement('div');
    document.body.appendChild(host);
    act(() => {
      render(
        <EnginePermissionBody threadId="t" toolUseId="tu-menu" question="Allow it?" options={options} request={request} />,
        host,
      );
    });
    const menu = host.querySelector('.permission-remember');
    expect(menu !== null).toBe(remembered.length > 1);
    const labels = Array.from(host.querySelectorAll('.permission-actions button'), button => button.textContent?.trim());
    if (remembered.length === 1) expect(labels).toContain(remembered[0].label);
    expect(host.querySelector('.question-options')).toBeNull();
    render(null, host);
    host.remove();
  });
});

describe('EnginePermissionBody', () => {
  let host: HTMLDivElement;

  beforeEach(() => {
    host = document.createElement('div');
    document.body.appendChild(host);
    answerThreadQuestion.mockClear();
  });

  afterEach(() => {
    render(null, host);
    host.remove();
    pendingAnswers.clear('tu-1');
  });

  const { options, request } = requestFor({ action: 'cancel', thisThread: true, always: true });

  function mount(props: Partial<Parameters<typeof EnginePermissionBody>[0]> = {}) {
    act(() => {
      render(
        <EnginePermissionBody threadId="t" toolUseId="tu-1" question="Cancel the thread?" options={options} request={request} {...props} />,
        host,
      );
    });
  }

  const buttonsByText = () => Array.from(host.querySelectorAll<HTMLButtonElement>('.permission-actions button'));

  it('renders through the permission shell, not the question body', () => {
    mount();
    expect(host.querySelector('.permission-body[data-request-id="tu-1"]')).not.toBeNull();
    expect(host.querySelector('.question-body')).toBeNull();
    expect(buttonsByText().map(button => button.textContent?.trim())).toEqual(['Remember', 'Deny', 'Allow once']);
  });

  it('answers with the option id when Allow once is tapped', () => {
    mount();
    act(() => { buttonsByText().find(button => button.textContent?.includes('Allow once'))?.click(); });
    expect(answerThreadQuestion).toHaveBeenCalledWith('t', 'tu-1', { kind: 'Selected', option_id: 'once' });
  });

  it('marks the picked button on an answered card and disables the row', () => {
    mount({ resolved: { kind: 'Selected', option_id: 'refuse' } });
    const deny = buttonsByText().find(button => button.textContent?.includes('Deny'));
    expect(deny?.getAttribute('aria-pressed')).toBe('true');
    expect(buttonsByText().every(button => button.disabled)).toBe(true);
  });

  it('shows a lone lasting choice as its own button, with no Remember menu', () => {
    const lone = requestFor({ action: 'grant-trigger-programs', thisThread: false, always: true });
    mount({ ...lone, resolved: undefined });
    expect(host.querySelector('.permission-remember')).toBeNull();
    expect(buttonsByText().map(button => button.textContent?.trim())).toEqual(['Always allow', 'Deny', 'Allow once']);
    act(() => { buttonsByText().find(button => button.textContent?.includes('Always allow'))?.click(); });
    expect(answerThreadQuestion).toHaveBeenCalledWith('t', 'tu-1', { kind: 'Selected', option_id: 'always' });
  });

  it('shows a card asked ahead with its lasting choices as buttons and its own Cancel', () => {
    const entries: [QuestionOption, PermissionChoice][] = [
      [{ id: 'always', label: 'Always allow' }, { kind: 'grant', lifetime: 'standing', holder: { kind: 'workspace' }, breadth: 'asked' }],
      [{ id: 'deny', label: 'Always deny' }, { kind: 'refuse_standing' }],
      [{ id: 'cancel', label: CANCEL_LABEL }, { kind: 'refuse' }],
    ];
    mount({
      options: entries.map(([option]) => option),
      request: { action: 'grant-trigger-programs', choices: Object.fromEntries(entries.map(([o, c]) => [o.id, c])) },
    });
    expect(host.querySelector('.permission-remember')).toBeNull();
    expect(buttonsByText().map(button => button.textContent?.trim())).toEqual(['Always allow', 'Always deny', 'Cancel']);
    const cancel = buttonsByText().find(button => button.textContent?.trim() === 'Cancel');
    expect(cancel?.classList.contains('action-btn-danger')).toBe(false);
    act(() => { cancel?.click(); });
    expect(answerThreadQuestion).toHaveBeenCalledWith('t', 'tu-1', { kind: 'Selected', option_id: 'cancel' });
  });

  it('labels Cancel as the engine does', () => {
    expect(MENU_RS).toContain(`pub const CANCEL_LABEL: &str = ${JSON.stringify(CANCEL_LABEL)};`);
  });

  it('keeps a lone lasting choice drawn, disabled, on a card answered otherwise', () => {
    mount({ ...requestFor({ action: 'apply', thisThread: true, always: false }), resolved: { kind: 'Selected', option_id: 'refuse' } });
    const labels = buttonsByText().map(button => button.textContent?.trim());
    expect(labels).toContain('Allow for this thread');
    expect(labels).not.toContain('Remember');
    expect(buttonsByText().every(button => button.disabled)).toBe(true);
  });

  it('shows a remembered pick in place of the Remember button once answered', () => {
    mount({ resolved: { kind: 'Selected', option_id: 'always' } });
    const picked = buttonsByText().find(button => button.getAttribute('aria-pressed') === 'true');
    expect(picked?.textContent).toContain('Always allow');
  });

  it('disables every button on a dead card', () => {
    mount({ terminated: true });
    expect(buttonsByText().every(button => button.disabled)).toBe(true);
  });

  it('says so when a closing message replaced the card', () => {
    mount({ resolved: { kind: 'Superseded' } });
    expect(host.querySelector('.permission-resolution-note')?.textContent).toBe('Replaced by your next message');
  });

  it('names the question it answers, so a typed reply lands on it', () => {
    mount();
    expect(host.querySelector('.permission-body')?.getAttribute('data-tool-use-id')).toBe('tu-1');
  });

  it('takes a typed answer still sending as its answer, drawn once', () => {
    mount({ typedAnswer: { state: 'sending', text: 'only on main', image_hashes: [] } });
    expect(buttonsByText().every(button => button.disabled)).toBe(true);
    expect(host.querySelectorAll('.question-freetext-text')).toHaveLength(1);
    expect(host.querySelector('.question-freetext-text')?.textContent).toContain('only on main');
  });
});

describe('the tool-server card reads its lasting choices from the same builder', () => {
  it('offers this thread, this tool and the whole server in the Remember menu', () => {
    const host = document.createElement('div');
    document.body.appendChild(host);
    act(() => {
      render(
        <McpPermissionBody
          event={{ request_id: 'req-mcp', tool_use_id: 'tu', server_id: 's', server_name: 'slack', tool_name: 'post_message', arguments_summary: '{}' }}
        />,
        host,
      );
    });
    expect(host.querySelector('.permission-remember')).not.toBeNull();
    render(null, host);
    host.remove();
  });
});
