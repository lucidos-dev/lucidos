// @vitest-environment jsdom
import { afterEach, beforeEach, describe, expect, it, vi } from 'vitest';
import { render } from 'preact';
import { act } from 'preact/test-utils';
import type { GatewayUser } from '../../../api/client/control';

vi.mock('../../../store/actions/gatewayUserManagement', async (original) => ({
  ...(await original<typeof import('../../../store/actions/gatewayUserManagement')>()),
  addEmailPrompt: vi.fn(),
  addGatewayUserPrompt: vi.fn(),
  removeEmail: vi.fn(),
  removeGatewayUserConfirmed: vi.fn(),
  renameGatewayUserPrompt: vi.fn(),
}));
const { openSettingsSubview } = vi.hoisted(() => ({ openSettingsSubview: vi.fn() }));
vi.mock('../../../store/actions/menu', () => ({ openSettingsSubview }));

import { UsersSection } from '../UsersSection';
import { gatewayUsers } from '../../../store/actions/gatewayUsers';
import {
  addEmailPrompt, addGatewayUserPrompt, removeEmail,
  removeGatewayUserConfirmed, renameGatewayUserPrompt,
} from '../../../store/actions/gatewayUserManagement';
import { NO_GATEWAY } from '../../../store/actions/pairedDevices';

const alex: GatewayUser = {
  id: 'alex', name: 'Alex Morgan', emails: ['alex@example.com', 'alex@example.org'], device_ids: ['laptop'],
};
const sam: GatewayUser = { id: 'sam', name: 'Sam Lee', emails: [], device_ids: ['phone', 'tablet'] };
let host: HTMLDivElement;

function button(name: string, root: ParentNode = host): HTMLButtonElement {
  const found = [...root.querySelectorAll<HTMLButtonElement>('button')]
    .find((el) => (el.getAttribute('aria-label') ?? el.textContent) === name);
  expect(found, `button ${name}`).toBeDefined();
  return found!;
}

beforeEach(() => {
  vi.clearAllMocks();
  gatewayUsers.value = { status: 'loaded', data: { users: [alex, sam], relayed: [] } };
  host = document.createElement('div');
  document.body.append(host);
  act(() => render(<UsersSection gateway={null} />, host));
});

afterEach(() => {
  act(() => render(null, host));
  host.remove();
  gatewayUsers.value = { status: 'not-loaded' };
});

describe('Settings Users', () => {
  it('opens Access from the pairing instructions', () => {
    button('Settings → Access').click();
    expect(openSettingsSubview).toHaveBeenCalledWith('access');
    expect(button('Settings → Access').parentElement?.classList.contains('list-row-details-prose')).toBe(true);
  });

  it('keeps rename beside the name and user removal accessible', () => {
    const rename = button('Rename Alex Morgan');
    expect(rename.parentElement?.textContent).toBe('Alex Morgan');
    expect(rename.classList.contains('inline-icon')).toBe(true);
    rename.click();
    expect(renameGatewayUserPrompt).toHaveBeenCalledWith(null, alex);

    const remove = button('Remove Alex Morgan');
    expect(remove.classList.contains('row-icon')).toBe(true);
    expect(remove.getAttribute('data-tooltip')).toContain('Revoke every device');
    remove.click();
    expect(removeGatewayUserConfirmed).toHaveBeenCalledWith(null, alex);
  });

  it('shows neutral email text with a separate removal control per address', () => {
    for (const email of alex.emails) {
      const remove = button(`Remove ${email}`);
      expect(remove.textContent).toBe('');
      expect(remove.classList.contains('inline-icon')).toBe(true);
      expect(remove.parentElement?.querySelector('.user-email-address')?.textContent).toBe(email);
      expect(remove.parentElement?.querySelector('.action-btn')).toBeNull();
      remove.click();
      expect(removeEmail).toHaveBeenLastCalledWith(null, alex, email);
    }
  });

  it('retains Add email for users with and without emails, plus Add a user', () => {
    const rows = [...host.querySelectorAll('.user-row')];
    expect(rows).toHaveLength(2);
    for (const [index, user] of [alex, sam].entries()) {
      expect(rows[index].textContent).toContain(`${user.device_ids.length} device${index ? 's' : ''}`);
      const add = button('Add email', rows[index]);
      expect(add.className).toBe('action-btn');
      expect(add.parentElement?.classList.contains('list-row-info')).toBe(true);
      add.click();
      expect(addEmailPrompt).toHaveBeenLastCalledWith(null, user);
    }
    button('Add a user').click();
    expect(addGatewayUserPrompt).toHaveBeenCalledOnce();
  });

  it('updates names, counts and addresses when the user list changes', () => {
    act(() => {
      gatewayUsers.value = { status: 'loaded', data: {
        users: [{ ...alex, name: 'Alex Renamed', emails: ['new@example.net'], device_ids: [] }, sam], relayed: [],
      } };
    });
    expect(host.textContent).toContain('0 devices');
    expect(host.textContent).not.toContain('alex@example.com');
    button('Rename Alex Renamed');
    button('Remove new@example.net').click();
    expect(removeEmail).toHaveBeenCalledWith(null, expect.objectContaining({ name: 'Alex Renamed' }), 'new@example.net');
  });

  it('preserves personal-gateway and absent-gateway states', () => {
    act(() => { gatewayUsers.value = { status: 'loaded', data: { users: [alex], relayed: [] } }; });
    expect(host.querySelector('.user-row')).toBeNull();
    expect(host.textContent).toContain('Sharing this machine with a team?');
    button('Add a user');

    act(() => { gatewayUsers.value = { status: 'loaded', data: NO_GATEWAY }; });
    expect(host.textContent).toContain('this engine runs without one');
    expect(host.querySelector('button')).toBeNull();
  });
});
