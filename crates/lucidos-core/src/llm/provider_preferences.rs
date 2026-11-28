use crate::core::prefs::{self, Flag, Pref};
use sqlx::PgPool;

pub const PROVIDER_CREDENTIAL_SERVICES: [&str; 5] =
    ["openai", "anthropic", "openrouter", "xai", "local"];

pub const PROVIDER_ENABLED_SWITCHES: [&Pref<Flag>; 8] = [
    &prefs::PROVIDER_ENABLED_VERTEX,
    &prefs::PROVIDER_ENABLED_ANTHROPIC,
    &prefs::PROVIDER_ENABLED_OPENAI,
    &prefs::PROVIDER_ENABLED_OPENROUTER,
    &prefs::PROVIDER_ENABLED_XAI,
    &prefs::PROVIDER_ENABLED_LOCAL,
    &prefs::PROVIDER_ENABLED_CLAUDE_GATEWAY,
    &prefs::PROVIDER_ENABLED_CHATGPT_PLAN,
];

/// INVARIANT: Preference keys that, when changed, change which LLM provider is installed.
/// The same subscriber that watches [`PROVIDER_CREDENTIAL_SERVICES`] filters on
/// this set, so a provider configured by a preference hot-swaps exactly like one
/// configured by a credential. `opencode-free` has no credential at all, and the
/// local base URL used to need a restart. The Claude gateway URL decides which
/// sign-in counts. The per-provider switches are here for the same reason: a
/// switch that needed a restart is a switch the user reads as broken.
pub const PROVIDER_PREFERENCE_KEYS: [&str; 11] = [
    prefs::OPENCODE_FREE_ENABLED.key(),
    prefs::LOCAL_BASE_URL.key(),
    prefs::CLAUDE_GATEWAY_URL.key(),
    PROVIDER_ENABLED_SWITCHES[0].key(),
    PROVIDER_ENABLED_SWITCHES[1].key(),
    PROVIDER_ENABLED_SWITCHES[2].key(),
    PROVIDER_ENABLED_SWITCHES[3].key(),
    PROVIDER_ENABLED_SWITCHES[4].key(),
    PROVIDER_ENABLED_SWITCHES[5].key(),
    PROVIDER_ENABLED_SWITCHES[6].key(),
    PROVIDER_ENABLED_SWITCHES[7].key(),
];

/// INVARIANT: The per-provider enable switches, resolved from the `provider_enabled_*`
/// preferences (Settings → Providers).
///
/// A switch is a VETO over a provider that is otherwise configured. It never
/// installs one, and it never touches the credential: turning a provider off is
/// how a user parks a key they still want stored.
///
/// Every field defaults to its catalog default (on), and only an explicit off
/// turns one off. A workspace that never opened the page therefore resolves
/// what it always did, and so does a boot that cannot read preferences at all.
#[derive(Debug, Clone, Copy)]
pub struct ProviderSwitches {
    pub vertex: bool,
    pub anthropic: bool,
    pub openai: bool,
    pub openrouter: bool,
    pub xai: bool,
    pub local: bool,
    pub claude_gateway: bool,
    pub chatgpt_plan: bool,
}

impl Default for ProviderSwitches {
    fn default() -> Self {
        Self {
            vertex: prefs::PROVIDER_ENABLED_VERTEX.default_flag(),
            anthropic: prefs::PROVIDER_ENABLED_ANTHROPIC.default_flag(),
            openai: prefs::PROVIDER_ENABLED_OPENAI.default_flag(),
            openrouter: prefs::PROVIDER_ENABLED_OPENROUTER.default_flag(),
            xai: prefs::PROVIDER_ENABLED_XAI.default_flag(),
            local: prefs::PROVIDER_ENABLED_LOCAL.default_flag(),
            claude_gateway: prefs::PROVIDER_ENABLED_CLAUDE_GATEWAY.default_flag(),
            chatgpt_plan: prefs::PROVIDER_ENABLED_CHATGPT_PLAN.default_flag(),
        }
    }
}

/// INVARIANT: Read the eight switches, defaulting any unreadable one to its default (on).
/// The handle read logs a failed read. Losing a provider over a transient query
/// error is worse than honouring the veto one build late.
pub async fn read_provider_switches(pool: &PgPool) -> ProviderSwitches {
    ProviderSwitches {
        vertex: prefs::PROVIDER_ENABLED_VERTEX.read(pool).await,
        anthropic: prefs::PROVIDER_ENABLED_ANTHROPIC.read(pool).await,
        openai: prefs::PROVIDER_ENABLED_OPENAI.read(pool).await,
        openrouter: prefs::PROVIDER_ENABLED_OPENROUTER.read(pool).await,
        xai: prefs::PROVIDER_ENABLED_XAI.read(pool).await,
        local: prefs::PROVIDER_ENABLED_LOCAL.read(pool).await,
        claude_gateway: prefs::PROVIDER_ENABLED_CLAUDE_GATEWAY.read(pool).await,
        chatgpt_plan: prefs::PROVIDER_ENABLED_CHATGPT_PLAN.read(pool).await,
    }
}
