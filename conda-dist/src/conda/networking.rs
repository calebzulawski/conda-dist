use anyhow::{Context, Result};
use rattler_networking::{AuthenticationMiddleware, LazyClient};
use reqwest_middleware::ClientBuilder;
use reqwest_middleware::reqwest::Client;
use std::sync::Arc;

const USER_AGENT: &str = concat!(env!("CARGO_PKG_NAME"), "/", env!("CARGO_PKG_VERSION"));

pub fn authenticated_client() -> Result<LazyClient> {
    let middleware =
        AuthenticationMiddleware::from_env_and_defaults().context("failed to load auth config")?;
    let client = Client::builder()
        .user_agent(USER_AGENT)
        .build()
        .context("failed to construct HTTP client")?;
    let client = ClientBuilder::new(client)
        .with_arc(Arc::new(middleware))
        .build();
    Ok(client.into())
}
