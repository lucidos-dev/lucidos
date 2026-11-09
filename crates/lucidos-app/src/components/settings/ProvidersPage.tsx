import { PREFERENCE_CATALOG } from '@lucidos/preference-catalog';
import { usePanelRefresh } from '../../hooks/usePanelRefresh';
import { loadClaudeGatewayStatus } from '../../store/actions/claudeGateway';
import { loadCredentials } from '../../store/actions/credentials';
import { currentVertexRegion, loadPreferences, setVertexRegion } from '../../store/actions/preferences';
import { preferences } from '../../store/store';
import { providerAnchor } from '../../store/modelProviders';
import { Dropdown } from '../shared/Dropdown';
import { AnthropicProviderSettings } from './AnthropicProviderSettings';
import { ClaudeGatewaySettings } from './ClaudeGatewaySettings';
import { CloudflareWorkersAiSettings } from './CloudflareWorkersAiSettings';
import { CustomSystemOneSettings } from './CustomSystemOneSettings';
import { LocalProviderSettings } from './LocalProviderSettings';
import { OpenAiProviderSettings } from './OpenAiProviderSettings';
import { OpenCodeFreeSettings } from './OpenCodeFreeSettings';
import { OpenRouterProviderSettings } from './OpenRouterProviderSettings';
import { ProviderBlock } from './ProviderBlock';
import { TypeSafeJudgmentSettings } from './TypeSafeJudgmentSettings';
import { XaiProviderSettings } from './XaiProviderSettings';

const VERTEX_REGIONS = [
  { value: 'global', label: 'global' },
  { value: 'eu', label: 'eu (multi-region)' },
  { value: 'europe-west1', label: 'europe-west1 (Belgium)' },
  { value: 'europe-west4', label: 'europe-west4 (Netherlands)' },
  { value: 'europe-west9', label: 'europe-west9 (Paris)' },
  { value: 'europe-north1', label: 'europe-north1 (Finland)' },
  { value: 'us-central1', label: 'us-central1 (Iowa)' },
  { value: 'us-east1', label: 'us-east1 (S. Carolina)' },
  { value: 'us-east4', label: 'us-east4 (N. Virginia)' },
  { value: 'us-east5', label: 'us-east5 (Columbus)' },
  { value: 'us-west1', label: 'us-west1 (Oregon)' },
  { value: 'us-west4', label: 'us-west4 (Las Vegas)' },
  { value: 'asia-southeast1', label: 'asia-southeast1 (Singapore)' },
  { value: 'asia-northeast1', label: 'asia-northeast1 (Tokyo)' },
  { value: 'asia-east1', label: 'asia-east1 (Taiwan)' },
];

/** The Vertex provider block. Unlike the other providers it has no credential
 *  to enter: it authenticates via ambient GCP Application Default Credentials
 *  (`gcloud auth application-default login`), so its only knob is the region. */
function VertexProviderSettings() {
  return (
    <ProviderBlock
      id="vertex"
      label="Vertex AI"
      anchor={providerAnchor('vertex')}
      // No credential to keep: Vertex is configured on the machine, by gcloud.
      // So its off state promises nothing about a stored key.
      hasStoredConfig={false}
      explainer={
        <>
          <p>
            Serves the Claude models (Opus / Sonnet / Haiku) on the{' '}
            <strong>vertex</strong> provider via Google Cloud.
          </p>
          <p>
            No key to enter: it uses your <strong>gcloud</strong> Application Default
            Credentials (<code>gcloud auth application-default login</code>). The region
            below is the only setting.
          </p>
        </>
      }
    >
      <div class="settings-row" data-search-anchor={providerAnchor('vertex', 'region')}>
        <span class="settings-row-label">Region</span>
        <Dropdown
          options={VERTEX_REGIONS}
          value={currentVertexRegion()}
          freeText
          placeholder={`e.g. ${PREFERENCE_CATALOG.vertex_region.fallback}`}
          onChange={setVertexRegion}
        />
      </div>
    </ProviderBlock>
  );
}

/** Settings → Providers: where every backend is set up, in two sections. The
 *  model providers serve the registry's models. System One providers answer
 *  typed judgments and serve none of them. */
export function ProvidersPage() {
  // Subscribe to preference changes, which hold each provider's switch.
  preferences.value;
  usePanelRefresh('providers settings', () => Promise.all([
    loadPreferences(),
    loadCredentials(),
    loadClaudeGatewayStatus(),
  ]));

  return (
    <>
      <div class="settings-section">
        <div class="settings-section-title">Model providers</div>
        <VertexProviderSettings />
        <AnthropicProviderSettings />
        <OpenAiProviderSettings />
        <OpenRouterProviderSettings />
        <XaiProviderSettings />
        <OpenCodeFreeSettings />
        <LocalProviderSettings />
        <ClaudeGatewaySettings />
      </div>
      <div class="settings-section">
        <div class="settings-section-title">System One</div>
        <TypeSafeJudgmentSettings />
        <CloudflareWorkersAiSettings />
        <CustomSystemOneSettings />
      </div>
    </>
  );
}
