import type { AuthType, CredentialInfo } from '../../store/types';
import { openEditCredential, deleteCredential } from '../../store/actions/credentials';
import { formatShortDate } from '../../utils/formatTime';
import { getCredentialValue } from '../../api/client';
import { showToast } from '../../store/store';
import { errorDetail } from '../../utils/errorDetail';

interface Props {
  credential: CredentialInfo;
}

function formatAuthType(authType: AuthType): string {
  switch (authType) {
    case 'api_key': return 'API Key';
    case 'bearer': return 'Bearer Token';
    case 'basic': return 'Basic Auth';
    case 'password': return 'Password';
    case 'oauth_client': return 'OAuth Client';
    case 'email_password': return 'Email Password';
    default: return authType;
  }
}

/** How a credential row titles and explains itself.
 *
 *  Two auth types read as ordinary rows without help. An `oauth_client` is the
 *  app registration behind a connected account, not a second account: a user who
 *  had just connected Dropbox saw it sitting next to a stray plain credential of
 *  the same name and could not tell which was which, or what either was for
 *  (2026-08-05). An `email_password` has the same shape.
 *
 *  The title is always the service name, which is now the provider or the
 *  mailbox account with no namespace prefix wrapped around it, and the `note`
 *  says what the row is. Keyed on `authType` rather than on how the name is
 *  spelled, which is the same correction the storage layer made: the type was
 *  always the thing being described, and the prefix was a second copy of it that
 *  could drift. Pure so the labelling is unit-testable without rendering. */
export function credentialRowLabel(
  serviceName: string,
  authType: AuthType
): { title: string; note: string | null } {
  if (authType === 'oauth_client') {
    return {
      title: serviceName,
      note: `App registration for the ${serviceName} connected account`,
    };
  }
  if (authType === 'email_password') {
    return { title: serviceName, note: 'Mailbox password' };
  }
  return { title: serviceName, note: null };
}

interface CopyTarget {
  label: string;
  jsonField?: string;
}

const COPY_TARGETS: Record<AuthType, CopyTarget[]> = {
  api_key: [{ label: 'Key' }],
  bearer: [{ label: 'Token' }],
  basic: [{ label: 'Value' }],
  password: [
    { label: 'Username', jsonField: 'username' },
    { label: 'Password', jsonField: 'password' },
  ],
  oauth_client: [
    { label: 'ID', jsonField: 'client_id' },
    { label: 'Secret', jsonField: 'client_secret' },
  ],
  email_password: [{ label: 'Password' }],
};

async function copyCredential(id: string, target: CopyTarget) {
  try {
    const { auth_value } = await getCredentialValue(id);
    let value: string;
    if (target.jsonField) {
      const parsed = JSON.parse(auth_value);
      value = parsed[target.jsonField] ?? '';
    } else {
      value = auth_value;
    }
    if (!value) {
      showToast(`No ${target.label} found`, 'error');
      return;
    }
    await navigator.clipboard.writeText(value);
    showToast(`${target.label} copied`);
  } catch (e) {
    showToast(`Failed to copy: ${errorDetail(e)}`, 'error');
  }
}

export function CredentialItem({ credential }: Props) {
  const dateStr = formatShortDate(new Date(credential.created_at));
  const copyTargets = COPY_TARGETS[credential.auth_type] ?? [];
  const { title, note } = credentialRowLabel(credential.service_name, credential.auth_type);

  return (
    <div class="list-row credential-row">
      <div class="list-row-info">
        <div class="title list-row-name">{title}</div>
        <div class="list-row-details">
          <span class="list-row-url">{credential.base_url}</span>
          <span class="list-row-type">{formatAuthType(credential.auth_type)}</span>
        </div>
        {/* A sentence, so `.list-row-details-prose` (the bare class is a flex
            row of FIELDS and would blow holes through it). */}
        {note && <div class="list-row-details list-row-details-prose">{note}</div>}
        <div class="list-row-date">Added {dateStr}</div>
      </div>
      <div class="list-row-actions">
        {copyTargets.map((target) => (
          <button
            key={target.label}
            class="action-btn"
            onClick={() => void copyCredential(credential.id, target)}
          >
            Copy {target.label}
          </button>
        ))}
        <button
          class="action-btn"
          onClick={() => openEditCredential(credential.id)}
        >
          Edit
        </button>
        <button
          class="action-btn action-btn-danger"
          onClick={() => void deleteCredential(credential.id, credential.service_name)}
        >
          Delete
        </button>
      </div>
    </div>
  );
}
