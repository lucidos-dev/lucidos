import type { AuthType, CredentialInfo } from '../../store/types';
import { openEditCredential, deleteCredential } from '../../store/actions/credentials';
import { formatShortDate } from '../../utils/formatTime';
import { getCredentialValue } from '../../api/client';
import { showToast } from '../../store/store';
import { errorDetail } from '../../utils/errorDetail';
import { copyToClipboard } from '../../utils/clipboard';
import { credentialAnchor } from './credentialAnchor';

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
    case 'secret': return 'Secret';
    case 'onepassword_service_account': return '1Password service account';
    default: return authType;
  }
}

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
  if (authType === 'secret') {
    return { title: serviceName, note: 'Shared secret, signed with rather than sent' };
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
  secret: [{ label: 'Secret' }],
  onepassword_service_account: [],
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
    copyToClipboard(value, `${target.label} copied`);
  } catch (e) {
    showToast(`Failed to copy: ${errorDetail(e)}`, 'error');
  }
}

export function CredentialItem({ credential }: Props) {
  const dateStr = formatShortDate(new Date(credential.created_at));
  const copyTargets = COPY_TARGETS[credential.auth_type] ?? [];
  const { title, note } = credentialRowLabel(credential.service_name, credential.auth_type);

  return (
    <div class="list-row credential-row" data-search-anchor={credentialAnchor(credential.id)}>
      <div class="list-row-info">
        <div class="title list-row-name">{title}</div>
        <div class="list-row-details">
          {credential.base_urls.map((url) => (
            <span class="list-row-url" key={url}>{url}</span>
          ))}
          <span class="list-row-type">{formatAuthType(credential.auth_type)}</span>
          {credential.secret_reference && <span class="list-row-type">1Password reference</span>}
        </div>
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
