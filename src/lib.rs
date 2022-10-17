pub use types::{ProjectData, ProjectKey};
use {
    async_trait::async_trait,
    reqwest::header::{self, HeaderValue},
    serde::de::DeserializeOwned,
    std::fmt::Debug,
    thiserror::Error as ThisError,
};

pub mod types;

const INVALID_TOKEN_ERROR: &str = "invalid auth token";

#[derive(Debug, ThisError)]
pub enum RegistryError {
    #[error("transport error: {0}")]
    Transport(#[from] reqwest::Error),

    #[error("invalid config: {0}")]
    Config(&'static str),

    #[error("invalid response: {0}")]
    Response(String),
}

pub type RegistryResult<T> = Result<T, RegistryError>;

#[async_trait]
pub trait RegistryClient: 'static + Send + Sync + Debug {
    async fn project_data(&self, id: &str) -> RegistryResult<Option<ProjectData>>;
}

#[derive(Debug, Clone)]
pub struct RegistryHttpClient {
    base_url: String,
    http_client: reqwest::Client,
}

impl RegistryHttpClient {
    pub fn new(base_url: impl Into<String>, auth_token: &str) -> RegistryResult<Self> {
        let mut auth_value = HeaderValue::from_str(&format!("Bearer {}", auth_token))
          .map_err(|_| RegistryError::Config(INVALID_TOKEN_ERROR))?;

        // Make sure we're not leaking auth token in debug output.
        auth_value.set_sensitive(true);

        let mut headers = header::HeaderMap::new();
        headers.insert(header::AUTHORIZATION, auth_value);

        let http_client = reqwest::Client::builder()
          .default_headers(headers)
          .build()?;

        Ok(Self {
            base_url: base_url.into(),
            http_client,
        })
    }
}

#[async_trait]
impl RegistryClient for RegistryHttpClient {
    async fn project_data(&self, id: &str) -> RegistryResult<Option<ProjectData>> {
        let resp = self
          .http_client
          .get(format!("{}/internal/project/key/{id}", self.base_url))
          .send()
          .await?;

        parse_http_response(resp).await
    }
}

async fn parse_http_response<T: DeserializeOwned>(
    resp: reqwest::Response,
) -> RegistryResult<Option<T>> {
    match resp.status().as_u16() {
        200..=299 => Ok(Some(resp.json().await?)),
        403 => Err(RegistryError::Config(INVALID_TOKEN_ERROR)),
        404 => Ok(None),
        _ => Err(RegistryError::Response(resp.status().to_string())),
    }
}
