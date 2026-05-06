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

async function copyField(serviceName: string, field: 'client_id' | 'client_secret') {
  try {
    const { auth_value } = await getCredentialValue(serviceName);
    const parsed = JSON.parse(auth_value);
    const value = parsed[field];
    if (!value) {
      showToast(`No ${field === 'client_id' ? 'Client ID' : 'Client Secret'} found`, 'error');
      return;
    }
    await navigator.clipboard.writeText(value);
    showToast(`${field === 'client_id' ? 'Client ID' : 'Client Secret'} copied`);
  } catch (e) {
    showToast(`Failed to copy: ${errorDetail(e)}`, 'error');
  }
}

export function CredentialItem({ credential }: Props) {
  const dateStr = formatShortDate(new Date(credential.created_at));

  return (
    <div class="list-row">
      <div class="list-row-info">
        <div class="title list-row-name">{credential.service_name}</div>
        <div class="list-row-details">
          <span class="list-row-url">{credential.base_url}</span>
          <span class="list-row-type">{formatAuthType(credential.auth_type)}</span>
        </div>
        <div class="list-row-date">Added {dateStr}</div>
      </div>
      <div class="list-row-actions">
        {credential.auth_type === 'oauth_client' && (
          <>
            <button
              class="action-btn"
              onClick={() => copyField(credential.service_name, 'client_id')}
            >
              Copy ID
            </button>
            <button
              class="action-btn"
              onClick={() => copyField(credential.service_name, 'client_secret')}
            >
              Copy Secret
            </button>
          </>
        )}
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
