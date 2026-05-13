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

async function copyCredential(serviceName: string, target: CopyTarget) {
  try {
    const { auth_value } = await getCredentialValue(serviceName);
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

  return (
    <div class="list-row credential-row">
      <div class="list-row-info">
        <div class="title list-row-name">{credential.service_name}</div>
        <div class="list-row-details">
          <span class="list-row-url">{credential.base_url}</span>
          <span class="list-row-type">{formatAuthType(credential.auth_type)}</span>
        </div>
        <div class="list-row-date">Added {dateStr}</div>
      </div>
      <div class="list-row-actions">
        {copyTargets.map((target) => (
          <button
            key={target.label}
            class="action-btn"
            onClick={() => copyCredential(credential.service_name, target)}
          >
            Copy {target.label}
          </button>
        ))}
        <button
          class="action-btn"
          onClick={() => openEditCredential(credential.service_name)}
        >
          Edit
        </button>
        <button
          class="action-btn action-btn-danger"
          onClick={() => deleteCredential(credential.service_name)}
        >
          Delete
        </button>
      </div>
    </div>
  );
}
