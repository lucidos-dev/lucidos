use std::future::Future;

use crate::core::preference_catalog::parse_public_https_url;

use super::funnel::FunnelState;

#[derive(Debug, Clone, PartialEq, Eq)]
pub(super) struct Route {
    pub host: String,
    pub port: u16,
    path_prefix: String,
}

impl Route {
    pub(super) fn delivery_path(&self, slug: &str, webhook_id: uuid::Uuid) -> String {
        format!("{}/{slug}/{webhook_id}", self.path_prefix)
    }
}

pub(super) async fn select_route<Fut>(
    stated: Result<Option<String>, sqlx::Error>,
    discover: impl FnOnce() -> Fut,
) -> Result<Option<Route>, &'static str>
where
    Fut: Future<Output = FunnelState>,
{
    match stated {
        Ok(Some(stated)) => {
            let url = parse_public_https_url(&stated)
                .map_err(|_| "the stored public webhook URL is invalid")?;
            return Ok(Some(Route {
                host: url.host_str().expect("validated HTTPS host").to_string(),
                port: url
                    .port_or_known_default()
                    .expect("HTTPS has a default port"),
                path_prefix: url.path().trim_end_matches('/').to_string(),
            }));
        }
        Ok(None) => {}
        Err(_) => return Err("the public webhook URL preference could not be read"),
    }

    match discover().await {
        FunnelState::Serving(ingress) => Ok(Some(Route {
            host: ingress.host,
            port: ingress.port,
            path_prefix: String::new(),
        })),
        FunnelState::NotServed => Ok(None),
        FunnelState::Unknown => Err("the funnel could not be read"),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::scheduler::webhook_ingress::funnel::PublicIngress;

    #[tokio::test]
    async fn stated_route_wins_and_keeps_its_prefix() {
        let route = select_route(
            Ok(Some("https://hooks.example.com:8443/relay%20path/".into())),
            || async { panic!("a stated URL bypasses discovery") },
        )
        .await
        .unwrap()
        .unwrap();
        assert_eq!(route.host, "hooks.example.com");
        assert_eq!(route.port, 8443);
        let id = uuid::Uuid::nil();
        assert_eq!(
            route.delivery_path("workspace", id),
            format!("/relay%20path/workspace/{id}")
        );
    }

    #[tokio::test]
    async fn route_selection_distinguishes_a_closed_gate_from_a_failed_read() {
        assert_eq!(
            select_route(Ok(None), || async { FunnelState::NotServed }).await,
            Ok(None),
        );
        assert!(select_route(Ok(None), || async { FunnelState::Unknown })
            .await
            .is_err());
        assert!(select_route(Err(sqlx::Error::PoolClosed), || async {
            panic!("failed preference read")
        })
        .await
        .is_err());
        let route = select_route(Ok(None), || async {
            FunnelState::Serving(PublicIngress {
                host: "hooks.example.com".into(),
                port: 443,
            })
        })
        .await
        .unwrap()
        .unwrap();
        assert!(route.path_prefix.is_empty());
    }
}
