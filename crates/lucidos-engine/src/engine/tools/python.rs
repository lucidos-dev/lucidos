use super::super::LucidosEngine;
use crate::core::oauth;
use crate::engine::event_bus::{BusEvent, SystemEvent};

impl LucidosEngine {
    /// Build CRED_* environment variables from all stored credentials.
    /// - password type: CRED_{NAME}_USERNAME and CRED_{NAME}_PASSWORD
    /// - other types (api_key, bearer, basic): CRED_{NAME} with the auth value
    pub(crate) fn credential_env_vars(
        credentials: Vec<crate::core::credentials::Credential>,
    ) -> Vec<(String, String)> {
        let mut env_vars = Vec::new();
        for cred in credentials {
            let env_name = cred
                .service_name
                .to_uppercase()
                .replace(['-', ' ', '.'], "_");
            let prefix = format!("CRED_{}", env_name);

            if cred.auth_type == crate::core::AuthType::Password {
                if let Ok(parsed) = serde_json::from_str::<serde_json::Value>(&cred.auth_value) {
                    let username = parsed["username"].as_str().unwrap_or("");
                    let password = parsed["password"].as_str().unwrap_or("");
                    env_vars.push((format!("{}_USERNAME", prefix), username.to_string()));
                    env_vars.push((format!("{}_PASSWORD", prefix), password.to_string()));
                }
            } else {
                env_vars.push((prefix, cred.auth_value));
            }
        }
        env_vars
    }

    /// Build OAUTH_* environment variables from connected OAuth accounts.
    /// For each account: OAUTH_{PROVIDER}_ACCESS_TOKEN (always), OAUTH_{PROVIDER}_EMAIL (if known).
    pub(crate) fn oauth_env_vars(accounts: Vec<oauth::OAuthAccount>) -> Vec<(String, String)> {
        let mut env_vars = Vec::new();
        for account in accounts {
            let provider = account
                .provider
                .to_uppercase()
                .replace(['-', ' ', '.'], "_");
            let prefix = format!("OAUTH_{}", provider);

            env_vars.push((format!("{}_ACCESS_TOKEN", prefix), account.access_token));
            if let Some(email) = account.email {
                env_vars.push((format!("{}_EMAIL", prefix), email));
            }
        }
        env_vars
    }

    pub(crate) async fn execute_python_tool(
        &self,
        args: &serde_json::Value,
    ) -> Result<String, Box<dyn std::error::Error + Send + Sync>> {
        let code = args["code"].as_str().unwrap_or("");
        let commit_message = args.get("commit_message").and_then(|v| v.as_str());

        if let Some(packages) = args.get("packages").and_then(|v| v.as_array()) {
            let names: Vec<&str> = packages.iter().filter_map(|p| p.as_str()).collect();
            if !names.is_empty() {
                if let Err(e) = self.python_runtime.ensure_packages(&names).await {
                    return Ok(format!("Error installing packages: {}", e));
                }
            }
        }

        let env_vars = self.build_script_env_vars().await;
        let run_id = uuid::Uuid::new_v4().to_string();
        let staging_dir = self.workspace_path().join(".lucidos/staging").join(&run_id);

        let output = match self
            .python_runtime
            .execute_staged(code, env_vars, &staging_dir)
            .await
        {
            Ok(output) => output,
            Err(e) => {
                std::fs::remove_dir_all(&staging_dir).ok();
                return Ok(format!("Error: {}", e));
            }
        };

        let data_staging = staging_dir.join("data");
        let mut created = Vec::new();
        let mut updated = Vec::new();

        if data_staging.exists() {
            let data_dir = self.workspace_path().join("data");
            Self::collect_staged_files(
                &data_staging,
                &data_staging,
                &data_dir,
                &mut created,
                &mut updated,
            )?;

            if !created.is_empty() || !updated.is_empty() {
                for path in created.iter().chain(updated.iter()) {
                    let src = data_staging.join(path);
                    let dst = data_dir.join(path);
                    if let Some(parent) = dst.parent() {
                        std::fs::create_dir_all(parent)?;
                    }
                    std::fs::copy(&src, &dst)?;
                }

                let all_paths: Vec<String> =
                    created.iter().chain(updated.iter()).cloned().collect();
                let message = commit_message.unwrap_or("Python script output");
                let commit_sha = self
                    .artifact_manager
                    .commit_data_paths(&all_paths, message)
                    .await
                    .map_err(|e| format!("Git commit failed: {}", e))?;

                for path in &created {
                    self.event_bus
                        .emit(BusEvent::System(SystemEvent::ArtifactCreated {
                            artifact_path: path.clone(),
                            commit: commit_sha.clone(),
                            source: Some("run_python".to_string()),
                        }))
                        .await?;
                }
                for path in &updated {
                    self.event_bus
                        .emit(BusEvent::System(SystemEvent::ArtifactUpdated {
                            artifact_path: path.clone(),
                            commit: commit_sha.clone(),
                            source: Some("run_python".to_string()),
                        }))
                        .await?;
                }
            }
        }

        std::fs::remove_dir_all(&staging_dir).ok();

        let mut response = output;
        if !created.is_empty() || !updated.is_empty() {
            response.push_str("\n\n[FILES]");
            for path in &created {
                response.push_str(&format!("\n  created: data/{}", path));
            }
            for path in &updated {
                response.push_str(&format!("\n  updated: data/{}", path));
            }
        }

        Ok(response)
    }

    /// Walk staging directory and classify files as created or updated.
    fn collect_staged_files(
        base: &std::path::Path,
        dir: &std::path::Path,
        workspace_data: &std::path::Path,
        created: &mut Vec<String>,
        updated: &mut Vec<String>,
    ) -> Result<(), Box<dyn std::error::Error + Send + Sync>> {
        if !dir.exists() {
            return Ok(());
        }
        for entry in std::fs::read_dir(dir)? {
            let entry = entry?;
            let path = entry.path();
            if path.is_dir() {
                Self::collect_staged_files(base, &path, workspace_data, created, updated)?;
            } else {
                let relative = path
                    .strip_prefix(base)
                    .map_err(|e| format!("Path strip failed: {}", e))?
                    .to_string_lossy()
                    .to_string();
                if workspace_data.join(&relative).exists() {
                    updated.push(relative);
                } else {
                    created.push(relative);
                }
            }
        }
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use chrono::Utc;
    use uuid::Uuid;

    fn make_account(provider: &str, email: Option<&str>, token: &str) -> oauth::OAuthAccount {
        oauth::OAuthAccount {
            id: Uuid::new_v4(),
            provider: provider.to_string(),
            email: email.map(String::from),
            display_name: None,
            access_token: token.to_string(),
            refresh_token: None,
            token_expiry: None,
            scopes: String::new(),
            created_at: Utc::now(),
            updated_at: Utc::now(),
        }
    }

    #[test]
    fn oauth_env_vars_injects_access_token_and_email() {
        let accounts = vec![make_account(
            "google",
            Some("user@gmail.com"),
            "ya29.test-token",
        )];
        let vars = LucidosEngine::oauth_env_vars(accounts);
        let map: std::collections::HashMap<_, _> = vars.into_iter().collect();

        assert_eq!(
            map.get("OAUTH_GOOGLE_ACCESS_TOKEN").unwrap(),
            "ya29.test-token"
        );
        assert_eq!(map.get("OAUTH_GOOGLE_EMAIL").unwrap(), "user@gmail.com");
    }

    #[test]
    fn oauth_env_vars_skips_email_when_none() {
        let accounts = vec![make_account("github", None, "ghp_test123")];
        let vars = LucidosEngine::oauth_env_vars(accounts);
        let map: std::collections::HashMap<_, _> = vars.into_iter().collect();

        assert_eq!(map.get("OAUTH_GITHUB_ACCESS_TOKEN").unwrap(), "ghp_test123");
        assert!(!map.contains_key("OAUTH_GITHUB_EMAIL"));
    }

    #[test]
    fn oauth_env_vars_normalizes_provider_name() {
        let accounts = vec![make_account("my-provider", None, "tok")];
        let vars = LucidosEngine::oauth_env_vars(accounts);
        let map: std::collections::HashMap<_, _> = vars.into_iter().collect();

        assert_eq!(map.get("OAUTH_MY_PROVIDER_ACCESS_TOKEN").unwrap(), "tok");
    }
}
