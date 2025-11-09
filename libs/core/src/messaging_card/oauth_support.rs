use anyhow::{Result, anyhow};
use greentic_types::TenantCtx;

use crate::messaging_card::types::{MessageCard, MessageCardKind};
use crate::oauth::{
    OauthClient, OauthRelayContext, StartLink, StartTransport, make_start_request,
};

pub async fn ensure_oauth_start_url<T: StartTransport>(
    card: &mut MessageCard,
    ctx: &TenantCtx,
    client: &OauthClient<T>,
    relay: Option<OauthRelayContext>,
) -> Result<()> {
    if !matches!(card.kind, MessageCardKind::Oauth) {
        return Ok(());
    }

    let oauth = card
        .oauth
        .as_mut()
        .ok_or_else(|| anyhow!("oauth card missing oauth block"))?;

    if oauth.start_url.is_some() {
        return Ok(());
    }

    let request = make_start_request(
        &oauth.provider,
        &oauth.scopes,
        oauth.resource.as_deref(),
        oauth.prompt.as_ref(),
        ctx,
        relay,
        oauth.metadata.as_ref(),
    );
    let start = client.build_start_url(&request).await?;

    let StartLink {
        url,
        connection_name,
    } = start;
    oauth.start_url = Some(url.to_string());
    if oauth.connection_name.is_none()
        && let Some(connection) = connection_name
    {
        oauth.connection_name = Some(connection);
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::messaging_card::types::{MessageCardKind, OauthCard, OauthProvider};
    use crate::oauth::oauth_client::StartResponse;
    use crate::oauth::{OauthStartRequest, StartLink};
    use greentic_types::{EnvId, TenantCtx, TenantId};
    use reqwest::Url;
    use serde_json::json;

    #[tokio::test]
    async fn ensure_sets_start_url_and_connection_name() {
        let ctx = tenant_ctx();
        let transport = TestTransport::with_link(
            "https://oauth.greentic.dev/oauth/start",
            StartLink {
                url: Url::parse("https://oauth.greentic.dev/start/abc123").unwrap(),
                connection_name: Some("m365".into()),
            },
        );
        let client = OauthClient::with_transport(
            transport,
            Url::parse("https://oauth.greentic.dev/").unwrap(),
        );
        let mut card = oauth_card(None);

        ensure_oauth_start_url(&mut card, &ctx, &client, None)
            .await
            .expect("hydrated oauth card");

        let oauth = card.oauth.expect("oauth payload");
        assert_eq!(
            oauth.start_url.as_deref(),
            Some("https://oauth.greentic.dev/start/abc123")
        );
        assert_eq!(oauth.connection_name.as_deref(), Some("m365"));
    }

    #[tokio::test]
    async fn existing_connection_name_is_preserved() {
        let ctx = tenant_ctx();
        let transport = TestTransport::with_link(
            "https://oauth.greentic.dev/oauth/start",
            StartLink {
                url: Url::parse("https://oauth.greentic.dev/start/custom").unwrap(),
                connection_name: Some("m365".into()),
            },
        );
        let client = OauthClient::with_transport(
            transport,
            Url::parse("https://oauth.greentic.dev/").unwrap(),
        );
        let mut card = oauth_card(Some("prewired"));

        ensure_oauth_start_url(&mut card, &ctx, &client, None)
            .await
            .expect("hydrated oauth card");

        let oauth = card.oauth.expect("oauth payload");
        assert_eq!(
            oauth.start_url.as_deref(),
            Some("https://oauth.greentic.dev/start/custom")
        );
        assert_eq!(oauth.connection_name.as_deref(), Some("prewired"));
    }

    fn tenant_ctx() -> TenantCtx {
        TenantCtx::new(EnvId("dev".into()), TenantId("acme".into()))
    }

    fn oauth_card(connection: Option<&str>) -> MessageCard {
        MessageCard {
            kind: MessageCardKind::Oauth,
            oauth: Some(OauthCard {
                provider: OauthProvider::Microsoft,
                scopes: vec!["User.Read".into()],
                resource: Some("https://graph.microsoft.com".into()),
                prompt: None,
                start_url: None,
                connection_name: connection.map(|c| c.into()),
                metadata: Some(json!({"tenant": "acme"})),
            }),
            ..Default::default()
        }
    }

    #[derive(Clone)]
    struct TestTransport {
        expected: String,
        link: StartLink,
    }

    impl TestTransport {
        fn with_link(expected: &str, link: StartLink) -> Self {
            Self {
                expected: expected.into(),
                link,
            }
        }
    }

    #[async_trait::async_trait]
    impl StartTransport for TestTransport {
        async fn post_start(&self, url: Url, _: &OauthStartRequest) -> Result<StartResponse> {
            assert_eq!(url.as_str(), self.expected);
            let payload = json!({
                "url": self.link.url.to_string(),
                "connection_name": self.link.connection_name.clone(),
            });
            let response =
                serde_json::from_value(payload).expect("mock start response construction");
            Ok(response)
        }
    }
}
