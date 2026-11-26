use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct PipelineConfig {
    pub pipeline: Vec<LayerConfig>,
    #[serde(default)]
    pub granted_capabilities: Vec<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(tag = "type", rename_all = "snake_case")]
pub enum LayerConfig {
    StaticCredential {
        kind: StaticKind,
        credential: String,
        #[serde(default)]
        header: Option<String>,
        #[serde(default)]
        param_name: Option<String>,
    },
    ScriptHandshake {
        #[serde(default)]
        credential: Option<String>,
        script: String,
        #[serde(default)]
        oauth_providers: Vec<String>,
    },
    HmacSigned {
        key_credential: String,
        secret_credential: String,
        #[serde(default = "default_key_header")]
        key_header: String,
        algorithm: crate::api::proxy::HmacAlgorithm,
        signed_payload: crate::api::proxy::HmacSignedPayload,
        #[serde(default = "default_signature_param")]
        signature_param: String,
        #[serde(default)]
        timestamp_param: Option<String>,
    },
    WasmSigner {
        module: String,
        #[serde(default)]
        credential_handles: Vec<CredentialHandleConfig>,
    },
}

#[derive(Debug, Clone, Copy, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum StaticKind {
    Bearer,
    ApiKey,
    Basic,
    QueryParam,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct CredentialHandleConfig {
    pub name: String,
    pub credential: String,
}

fn default_key_header() -> String {
    "X-API-KEY".into()
}

fn default_signature_param() -> String {
    "signature".into()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn pipeline_config_parses_static_credential_bearer() {
        let json = r#"{
            "pipeline": [
                {"type": "static_credential", "kind": "bearer", "credential": "comfort"}
            ]
        }"#;
        let cfg: PipelineConfig = serde_json::from_str(json).unwrap();
        assert_eq!(cfg.pipeline.len(), 1);
        match &cfg.pipeline[0] {
            LayerConfig::StaticCredential {
                kind, credential, ..
            } => {
                assert_eq!(*kind, StaticKind::Bearer);
                assert_eq!(credential, "comfort");
            }
            other => panic!("expected StaticCredential, got {other:?}"),
        }
    }

    #[test]
    fn pipeline_config_parses_static_credential_api_key_with_header() {
        let json = r#"{
            "pipeline": [
                {"type": "static_credential", "kind": "api_key", "credential": "k", "header": "X-API-Key"}
            ]
        }"#;
        let cfg: PipelineConfig = serde_json::from_str(json).unwrap();
        match &cfg.pipeline[0] {
            LayerConfig::StaticCredential { kind, header, .. } => {
                assert_eq!(*kind, StaticKind::ApiKey);
                assert_eq!(header.as_deref(), Some("X-API-Key"));
            }
            other => panic!("expected StaticCredential, got {other:?}"),
        }
    }

    #[test]
    fn pipeline_config_parses_query_param_with_param_name() {
        let json = r#"{
            "pipeline": [
                {"type": "static_credential", "kind": "query_param", "credential": "k", "param_name": "api-key"}
            ]
        }"#;
        let cfg: PipelineConfig = serde_json::from_str(json).unwrap();
        match &cfg.pipeline[0] {
            LayerConfig::StaticCredential {
                kind, param_name, ..
            } => {
                assert_eq!(*kind, StaticKind::QueryParam);
                assert_eq!(param_name.as_deref(), Some("api-key"));
            }
            _ => panic!(),
        }
    }

    #[test]
    fn pipeline_config_parses_hmac_signed() {
        let json = r#"{
            "pipeline": [
                {"type": "hmac_signed",
                 "key_credential": "k",
                 "secret_credential": "s",
                 "key_header": "X-MBX-APIKEY",
                 "algorithm": "sha256",
                 "signed_payload": "query_string",
                 "signature_param": "signature",
                 "timestamp_param": "timestamp"}
            ]
        }"#;
        let cfg: PipelineConfig = serde_json::from_str(json).unwrap();
        assert!(matches!(cfg.pipeline[0], LayerConfig::HmacSigned { .. }));
    }

    #[test]
    fn pipeline_config_parses_script_handshake() {
        let json = r#"{
            "pipeline": [
                {"type": "script_handshake", "credential": "comfort", "script": "scripts/x.py"}
            ]
        }"#;
        let cfg: PipelineConfig = serde_json::from_str(json).unwrap();
        match &cfg.pipeline[0] {
            LayerConfig::ScriptHandshake {
                credential,
                script,
                oauth_providers,
            } => {
                assert_eq!(credential.as_deref(), Some("comfort"));
                assert_eq!(script, "scripts/x.py");
                assert!(oauth_providers.is_empty());
            }
            _ => panic!(),
        }
    }

    #[test]
    fn pipeline_config_parses_script_handshake_without_credential() {
        let json = r#"{
            "pipeline": [
                {"type": "script_handshake", "script": "scripts/auth/keychain-login.py"}
            ]
        }"#;
        let cfg: PipelineConfig = serde_json::from_str(json).unwrap();
        match &cfg.pipeline[0] {
            LayerConfig::ScriptHandshake {
                credential,
                script,
                oauth_providers,
            } => {
                assert!(credential.is_none(), "omitted credential should be None");
                assert_eq!(script, "scripts/auth/keychain-login.py");
                assert!(oauth_providers.is_empty());
            }
            _ => panic!(),
        }
    }

    #[test]
    fn pipeline_config_parses_script_handshake_with_oauth_providers() {
        let json = r#"{
            "pipeline": [
                {"type": "script_handshake",
                 "credential": "firebase-key",
                 "script": "scripts/auth/firebase.py",
                 "oauth_providers": ["google", "microsoft"]}
            ]
        }"#;
        let cfg: PipelineConfig = serde_json::from_str(json).unwrap();
        match &cfg.pipeline[0] {
            LayerConfig::ScriptHandshake {
                credential,
                script,
                oauth_providers,
            } => {
                assert_eq!(credential.as_deref(), Some("firebase-key"));
                assert_eq!(script, "scripts/auth/firebase.py");
                assert_eq!(
                    oauth_providers,
                    &vec!["google".to_string(), "microsoft".to_string()]
                );
            }
            _ => panic!(),
        }
    }

    #[test]
    fn pipeline_config_parses_wasm_signer_with_credential_handles() {
        let json = r#"{
            "pipeline": [
                {"type": "wasm_signer",
                 "module": "binance-hmac",
                 "credential_handles": [
                     {"name": "api_secret", "credential": "binance-secret"}
                 ]}
            ],
            "granted_capabilities": ["replace_body"]
        }"#;
        let cfg: PipelineConfig = serde_json::from_str(json).unwrap();
        assert_eq!(cfg.granted_capabilities, vec!["replace_body".to_string()]);
        match &cfg.pipeline[0] {
            LayerConfig::WasmSigner {
                module,
                credential_handles,
            } => {
                assert_eq!(module, "binance-hmac");
                assert_eq!(credential_handles.len(), 1);
                assert_eq!(credential_handles[0].name, "api_secret");
                assert_eq!(credential_handles[0].credential, "binance-secret");
            }
            _ => panic!(),
        }
    }

    #[test]
    fn pipeline_config_compose_script_handshake_then_wasm_signer() {
        let json = r#"{
            "pipeline": [
                {"type": "script_handshake", "credential": "comfort-pw", "script": "scripts/comfort-login.py"},
                {"type": "wasm_signer", "module": "comfort-cloud-hmac",
                 "credential_handles": [{"name": "api_secret", "credential": "comfort-secret"}]}
            ]
        }"#;
        let cfg: PipelineConfig = serde_json::from_str(json).unwrap();
        assert_eq!(cfg.pipeline.len(), 2);
        assert!(matches!(
            cfg.pipeline[0],
            LayerConfig::ScriptHandshake { .. }
        ));
        assert!(matches!(cfg.pipeline[1], LayerConfig::WasmSigner { .. }));
    }
}
