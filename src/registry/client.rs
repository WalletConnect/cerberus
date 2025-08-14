use {
    crate::{
        project::{
            FeaturesResponse, PlanLimits, ProjectData, ProjectDataWithLimits,
            ProjectDataWithLimitsAndFeatures, ProjectDataWithQuota,
        },
        registry::error::RegistryError,
    },
    async_trait::async_trait,
    reqwest::{
        header::{self, HeaderValue},
        IntoUrl, StatusCode, Url,
    },
    serde::{de::DeserializeOwned, Deserialize, Serialize},
    std::{fmt::Debug, time::Duration},
};

#[derive(Debug, Serialize, Deserialize, PartialEq, Eq, Hash, Clone)]
#[serde(rename_all = "camelCase")]
pub struct LimitsResponse {
    pub plan_limits: PlanLimits,
}

use once_cell::sync::Lazy;

static INTERNAL_API_BASE_URI: Lazy<Url> =
    Lazy::new(|| Url::parse("https://api.reown.com").expect("Invalid internal API base URI"));
const INVALID_TOKEN_ERROR: &str = "invalid auth token";

pub type RegistryResult<T> = Result<T, RegistryError>;

#[async_trait]
pub trait RegistryClient: 'static + Send + Sync + Debug {
    async fn project_data(&self, id: &str) -> RegistryResult<Option<ProjectData>>;
    async fn project_data_with_quota(
        &self,
        id: &str,
    ) -> RegistryResult<Option<ProjectDataWithQuota>>;
    async fn project_limits(&self, id: &str) -> RegistryResult<Option<LimitsResponse>>;
    async fn project_data_with_limits(
        &self,
        id: &str,
    ) -> RegistryResult<Option<ProjectDataWithLimits>>;
    async fn project_features(&self, id: &str) -> RegistryResult<Option<FeaturesResponse>>;
    async fn project_data_with_limits_and_features(
        &self,
        id: &str,
    ) -> RegistryResult<Option<ProjectDataWithLimitsAndFeatures>>;
}

/// HTTP client configuration.
#[derive(Debug, Clone)]
pub struct HttpClientConfig {
    /// Connection keep-alive timeout after being returned to the pool.
    ///
    /// `None` disables the timeout. Default is 90 seconds.
    pub pool_idle_timeout: Option<Duration>,

    /// Maximum number of idle connections to keep alive.
    ///
    /// Default is unlimited.
    pub pool_max_idle: usize,

    /// Enables a request timeout.
    ///
    /// The timeout is applied for both the connect phase of a `Client`, and for
    /// fully receiving response body.
    ///
    /// Default is no timeout.
    pub timeout: Option<Duration>,
}

impl Default for HttpClientConfig {
    fn default() -> Self {
        // These defaults are taken from `reqwest` default config.
        Self {
            pool_idle_timeout: Some(Duration::from_secs(90)),
            pool_max_idle: usize::MAX,
            timeout: None,
        }
    }
}

#[derive(Debug, Clone)]
pub struct RegistryHttpClient {
    base_explorer_url: Url,
    base_internal_api_url: Url,
    http_client: reqwest::Client,
    st: String,
    sv: String,
}

impl RegistryHttpClient {
    pub fn new(
        base_explorer_url: impl IntoUrl,
        auth_token: &str,
        origin: &str,
        st: &str,
        sv: &str,
    ) -> RegistryResult<Self> {
        Self::with_config(
            base_explorer_url,
            None::<&str>,
            auth_token,
            origin,
            st,
            sv,
            Default::default(),
        )
    }

    pub fn with_config(
        base_explorer_url: impl IntoUrl,
        base_internal_api_url: Option<impl IntoUrl>,
        auth_token: &str,
        origin: &str,
        st: &str,
        sv: &str,
        config: HttpClientConfig,
    ) -> RegistryResult<Self> {
        let mut auth_value = HeaderValue::from_str(&format!("Bearer {auth_token}"))
            .map_err(|_| RegistryError::Config(INVALID_TOKEN_ERROR))?;

        // Make sure we're not leaking auth token in debug output.
        auth_value.set_sensitive(true);

        let mut headers = header::HeaderMap::new();
        headers.insert(header::AUTHORIZATION, auth_value);
        headers.insert(
            header::ORIGIN,
            HeaderValue::from_str(origin).map_err(RegistryError::OriginParse)?,
        );

        // We can use the same client for both explorer and internal API
        // because the internal API is protected by the same auth token.
        let mut http_client = reqwest::Client::builder()
            .default_headers(headers)
            .pool_idle_timeout(config.pool_idle_timeout)
            .pool_max_idle_per_host(config.pool_max_idle);

        if let Some(timeout) = config.timeout {
            http_client = http_client.connect_timeout(timeout).timeout(timeout);
        }

        let internal_api_url = match base_internal_api_url {
            Some(url) => url.into_url().map_err(RegistryError::BaseUrlIntoUrl)?,
            None => INTERNAL_API_BASE_URI.clone(),
        };

        Ok(Self {
            base_explorer_url: base_explorer_url
                .into_url()
                .map_err(RegistryError::BaseUrlIntoUrl)?,
            base_internal_api_url: internal_api_url,
            http_client: http_client.build().map_err(RegistryError::BuildClient)?,
            st: st.to_string(),
            sv: sv.to_string(),
        })
    }

    async fn project_data_impl<T: DeserializeOwned>(
        &self,
        project_id: &str,
        quota: bool,
    ) -> RegistryResult<Option<T>> {
        if !is_valid_project_id(project_id) {
            return Ok(None);
        }

        let url = build_explorer_url(&self.base_explorer_url, project_id, quota)
            .map_err(RegistryError::UrlBuild)?;

        let resp = self
            .http_client
            .get(url)
            .send()
            .await
            .map_err(RegistryError::Transport)?;

        parse_http_response(resp).await
    }

    async fn project_limits_impl<T: DeserializeOwned>(
        &self,
        project_id: &str,
    ) -> RegistryResult<Option<T>> {
        if !is_valid_project_id(project_id) {
            return Ok(None);
        }

        let url =
            build_internal_api_url(&self.base_internal_api_url, project_id, &self.st, &self.sv)
                .map_err(RegistryError::UrlBuild)?;

        let resp = self
            .http_client
            .get(url)
            .send()
            .await
            .map_err(RegistryError::Transport)?;

        parse_http_response(resp).await
    }

    async fn project_data_with_limits_impl(
        &self,
        project_id: &str,
    ) -> RegistryResult<Option<ProjectDataWithLimits>> {
        if !is_valid_project_id(project_id) {
            return Ok(None);
        }
        let data: ProjectData = match self.project_data(project_id).await? {
            Some(project_data) => project_data,
            None => return Ok(None),
        };
        let limits: PlanLimits = match self.project_limits(project_id).await? {
            Some(response) => response.plan_limits,
            None => return Ok(None),
        };

        Ok(Some(ProjectDataWithLimits { data, limits }))
    }

    async fn project_features_impl<T: DeserializeOwned>(
        &self,
        project_id: &str,
    ) -> RegistryResult<Option<T>> {
        if !is_valid_project_id(project_id) {
            return Ok(None);
        }

        let url = build_features_url(&self.base_internal_api_url, project_id, &self.st, &self.sv)
            .map_err(RegistryError::UrlBuild)?;

        let resp = self
            .http_client
            .get(url)
            .send()
            .await
            .map_err(RegistryError::Transport)?;

        parse_http_response(resp).await
    }

    async fn project_data_with_limits_and_features_impl(
        &self,
        project_id: &str,
    ) -> RegistryResult<Option<ProjectDataWithLimitsAndFeatures>> {
        let data_with_limits = match self.project_data_with_limits_impl(project_id).await? {
            Some(data_with_limits) => data_with_limits,
            None => return Ok(None),
        };

        let features_response: FeaturesResponse = match self.project_features(project_id).await? {
            Some(response) => response,
            None => return Ok(None),
        };

        Ok(Some(ProjectDataWithLimitsAndFeatures {
            data: data_with_limits.data,
            limits: data_with_limits.limits,
            features: features_response.features,
        }))
    }
}

#[async_trait]
impl RegistryClient for RegistryHttpClient {
    async fn project_data(&self, project_id: &str) -> RegistryResult<Option<ProjectData>> {
        self.project_data_impl(project_id, false).await
    }

    async fn project_data_with_quota(
        &self,
        project_id: &str,
    ) -> RegistryResult<Option<ProjectDataWithQuota>> {
        self.project_data_impl(project_id, true).await
    }

    async fn project_limits(&self, project_id: &str) -> RegistryResult<Option<LimitsResponse>> {
        self.project_limits_impl(project_id).await
    }

    async fn project_data_with_limits(
        &self,
        project_id: &str,
    ) -> RegistryResult<Option<ProjectDataWithLimits>> {
        self.project_data_with_limits_impl(project_id).await
    }

    async fn project_features(&self, project_id: &str) -> RegistryResult<Option<FeaturesResponse>> {
        self.project_features_impl(project_id).await
    }

    async fn project_data_with_limits_and_features(
        &self,
        project_id: &str,
    ) -> RegistryResult<Option<ProjectDataWithLimitsAndFeatures>> {
        self.project_data_with_limits_and_features_impl(project_id).await
    }
}

fn build_explorer_url(
    base_url: &Url,
    project_id: &str,
    quota: bool,
) -> Result<Url, url::ParseError> {
    let mut url = base_url.join(&format!("/internal/project/key/{project_id}"))?;
    if quota {
        url.query_pairs_mut().append_pair("quotas", "true");
    }
    Ok(url)
}

fn build_internal_api_url(
    base_url: &Url,
    project_id: &str,
    st: &str,
    sv: &str,
) -> Result<Url, url::ParseError> {
    let mut url = base_url.join("/internal/v1/project-limits")?;
    url.query_pairs_mut().append_pair("projectId", project_id);
    url.query_pairs_mut().append_pair("st", st);
    url.query_pairs_mut().append_pair("sv", sv);
    Ok(url)
}

fn build_features_url(
    base_url: &Url,
    project_id: &str,
    st: &str,
    sv: &str,
) -> Result<Url, url::ParseError> {
    let mut url = base_url.join("/appkit/v1/config")?;
    url.query_pairs_mut().append_pair("projectId", project_id);
    url.query_pairs_mut().append_pair("st", st);
    url.query_pairs_mut().append_pair("sv", sv);
    Ok(url)
}

/// Checks if the project ID is formatted properly. It must be 32 hex
/// characters.
fn is_valid_project_id(project_id: &str) -> bool {
    project_id.len() == 32 && is_hex_string(project_id)
}

fn is_hex_string(string: &str) -> bool {
    string.chars().all(|c| c.is_ascii_hexdigit())
}

async fn parse_http_response<T: DeserializeOwned>(
    resp: reqwest::Response,
) -> RegistryResult<Option<T>> {
    let status = resp.status();
    match status {
        code if code.is_success() => Ok(Some(
            resp.json()
                .await
                .map_err(RegistryError::ResponseJsonParse)?,
        )),
        StatusCode::UNAUTHORIZED => Err(RegistryError::Config(INVALID_TOKEN_ERROR)),
        StatusCode::NOT_FOUND => Ok(None),
        _ => Err(RegistryError::Response(format!(
            "status={status} body={:?}",
            resp.text().await
        ))),
    }
}

#[cfg(test)]
mod test {
    use {
        super::*,
        crate::project::Quota,
        wiremock::{
            http::Method,
            matchers::{method, path, query_param},
            Mock, MockServer, ResponseTemplate,
        },
    };

    const TEST_ORIGIN: &str = "https://cerberus-tests.reown.com";

    fn mock_project_data() -> ProjectData {
        ProjectData {
            uuid: "".to_owned(),
            creator: "".to_owned(),
            name: "".to_owned(),
            push_url: None,
            keys: vec![],
            is_enabled: false,
            is_verify_enabled: false,
            is_rate_limited: false,
            allowed_origins: vec![],
            verified_domains: vec![],
            bundle_ids: vec![],
            package_names: vec![],
        }
    }

    #[tokio::test]
    async fn project_exists() {
        let project_id = "a".repeat(32);

        let mock_server = MockServer::start().await;

        Mock::given(method(Method::Get))
            .and(path(format!("/internal/project/key/{project_id}")))
            .respond_with(ResponseTemplate::new(StatusCode::OK).set_body_json(mock_project_data()))
            .mount(&mock_server)
            .await;

        let response = RegistryHttpClient::new(mock_server.uri(), "auth", TEST_ORIGIN, "st", "sv")
            .unwrap()
            .project_data(&project_id)
            .await
            .unwrap();
        assert!(response.is_some());
    }

    fn mock_project_data_quota() -> ProjectDataWithQuota {
        ProjectDataWithQuota {
            project_data: mock_project_data(),
            quota: Quota {
                max: 42,
                current: 1,
                is_valid: true,
            },
        }
    }

    #[tokio::test]
    async fn project_exists_quota() {
        let project_id = "a".repeat(32);

        let mock_server = MockServer::start().await;

        Mock::given(method(Method::Get))
            .and(path(format!("/internal/project/key/{project_id}")))
            .and(query_param("quotas", "true"))
            .respond_with(
                ResponseTemplate::new(StatusCode::OK).set_body_json(mock_project_data_quota()),
            )
            .mount(&mock_server)
            .await;

        let response = RegistryHttpClient::new(mock_server.uri(), "auth", TEST_ORIGIN, "st", "sv")
            .unwrap()
            .project_data_with_quota(&project_id)
            .await
            .unwrap();
        assert!(response.is_some());
    }

    #[tokio::test]
    async fn project_id_invalid_register() {
        let project_id = "a".repeat(32);

        let mock_server = MockServer::start().await;

        Mock::given(method(Method::Get))
            .and(path(format!("/internal/project/key/{project_id}")))
            .respond_with(ResponseTemplate::new(404))
            .mount(&mock_server)
            .await;

        let response = RegistryHttpClient::new(mock_server.uri(), "auth", TEST_ORIGIN, "st", "sv")
            .unwrap()
            .project_data(&project_id)
            .await
            .unwrap();
        assert!(response.is_none());
    }

    #[tokio::test]
    async fn project_id_invalid_len() {
        let project_id = "a".repeat(31);

        let mock_server = MockServer::start().await;

        let response = RegistryHttpClient::new(mock_server.uri(), "auth", TEST_ORIGIN, "st", "sv")
            .unwrap()
            .project_data(&project_id)
            .await
            .unwrap();
        assert!(response.is_none());
    }

    #[tokio::test]
    async fn project_id_invalid_len_long() {
        let project_id = "a".repeat(33);

        let mock_server = MockServer::start().await;

        let response = RegistryHttpClient::new(mock_server.uri(), "auth", TEST_ORIGIN, "st", "sv")
            .unwrap()
            .project_data(&project_id)
            .await
            .unwrap();
        assert!(response.is_none());
    }

    #[tokio::test]
    async fn project_id_invalid_hex() {
        let project_id = "z".repeat(32);

        let mock_server = MockServer::start().await;

        let response = RegistryHttpClient::new(mock_server.uri(), "auth", TEST_ORIGIN, "st", "sv")
            .unwrap()
            .project_data(&project_id)
            .await
            .unwrap();
        assert!(response.is_none());
    }

    #[tokio::test]
    async fn invalid_auth() {
        let project_id = "a".repeat(32);

        let mock_server = MockServer::start().await;

        Mock::given(method(Method::Get))
            .and(path(format!("/internal/project/key/{project_id}")))
            .respond_with(ResponseTemplate::new(StatusCode::UNAUTHORIZED))
            .mount(&mock_server)
            .await;

        let result = RegistryHttpClient::new(mock_server.uri(), "auth", TEST_ORIGIN, "st", "sv")
            .unwrap()
            .project_data(&project_id)
            .await;
        assert!(matches!(
            result,
            RegistryResult::Err(RegistryError::Config(INVALID_TOKEN_ERROR))
        ));
    }

    #[test]
    fn test_build_explorer_url() {
        let base_url = Url::parse("http://example.com").unwrap();
        let project_id = "a".repeat(32);

        let url = build_explorer_url(&base_url, &project_id, false).unwrap();
        assert_eq!(
            url.as_str(),
            "http://example.com/internal/project/key/aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa"
        );
    }

    #[test]
    fn test_build_url_quota() {
        let base_url = Url::parse("http://example.com").unwrap();
        let project_id = "a".repeat(32);

        let url = build_explorer_url(&base_url, &project_id, true).unwrap();
        assert_eq!(
            url.as_str(),
            "http://example.com/internal/project/key/aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa?quotas=true"
        );
    }

    #[test]
    fn test_build_features_url() {
        let base_url = Url::parse("http://example.com").unwrap();
        let project_id = "a".repeat(32);

        let url = build_features_url(&base_url, &project_id, "blockchain-api", "1.0.0").unwrap();
        assert_eq!(
            url.as_str(),
            "http://example.com/appkit/v1/config?projectId=aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa&st=blockchain-api&sv=1.0.0"
        );
    }

    fn mock_features_response() -> FeaturesResponse {
        FeaturesResponse {
            features: vec![
                crate::project::Feature {
                    id: "multi_wallet".to_string(),
                    is_enabled: false,
                    config: Some(serde_json::json!([])),
                },
                crate::project::Feature {
                    id: "social_login".to_string(),
                    is_enabled: true,
                    config: None,
                },
            ],
        }
    }

    #[tokio::test]
    async fn project_features_exist() {
        let project_id = "a".repeat(32);
        let mock_server = MockServer::start().await;

        Mock::given(method(Method::Get))
            .and(path("/appkit/v1/config"))
            .and(query_param("projectId", project_id.clone()))
            .and(query_param("st", "st"))
            .and(query_param("sv", "sv"))
            .respond_with(
                ResponseTemplate::new(StatusCode::OK).set_body_json(mock_features_response()),
            )
            .mount(&mock_server)
            .await;

        let response = RegistryHttpClient::with_config(
            mock_server.uri(),
            Some(mock_server.uri()),
            "auth",
            TEST_ORIGIN,
            "st",
            "sv",
            Default::default(),
        )
        .unwrap()
        .project_features(&project_id)
        .await
        .unwrap();
        assert!(response.is_some());
        let features = response.unwrap();
        assert_eq!(features.features.len(), 2);
        assert_eq!(features.features[0].id, "multi_wallet");
        assert!(!features.features[0].is_enabled);
        assert_eq!(features.features[1].id, "social_login");
        assert!(features.features[1].is_enabled);
    }

    #[tokio::test]
    async fn project_data_with_limits_and_features_exists() {
        let project_id = "a".repeat(32);
        let mock_server = MockServer::start().await;

        // Mock project data endpoint
        Mock::given(method(Method::Get))
            .and(path(format!("/internal/project/key/{project_id}")))
            .respond_with(ResponseTemplate::new(StatusCode::OK).set_body_json(mock_project_data()))
            .mount(&mock_server)
            .await;

        // Mock project limits endpoint
        Mock::given(method(Method::Get))
            .and(path("/internal/v1/project-limits"))
            .and(query_param("projectId", project_id.clone()))
            .and(query_param("st", "st"))
            .and(query_param("sv", "sv"))
            .respond_with(
                ResponseTemplate::new(StatusCode::OK).set_body_json(LimitsResponse {
                    plan_limits: crate::project::PlanLimits {
                        tier: "free".to_string(),
                        is_above_rpc_limit: false,
                        is_above_mau_limit: false,
                    },
                }),
            )
            .mount(&mock_server)
            .await;

        // Mock features endpoint
        Mock::given(method(Method::Get))
            .and(path("/appkit/v1/config"))
            .and(query_param("projectId", project_id.clone()))
            .and(query_param("st", "st"))
            .and(query_param("sv", "sv"))
            .respond_with(
                ResponseTemplate::new(StatusCode::OK).set_body_json(mock_features_response()),
            )
            .mount(&mock_server)
            .await;

        let response = RegistryHttpClient::with_config(
            mock_server.uri(),
            Some(mock_server.uri()),
            "auth",
            TEST_ORIGIN,
            "st",
            "sv",
            Default::default(),
        )
        .unwrap()
        .project_data_with_limits_and_features(&project_id)
        .await
        .unwrap();

        assert!(response.is_some());
        let data = response.unwrap();
        assert_eq!(data.limits.tier, "free");
        assert!(!data.limits.is_above_rpc_limit);
        assert!(!data.limits.is_above_mau_limit);
        assert_eq!(data.features.len(), 2);
        assert_eq!(data.features[0].id, "multi_wallet");
        assert_eq!(data.features[1].id, "social_login");
        assert!(!data.features[0].is_enabled);
        assert!(data.features[1].is_enabled);
    }
}
