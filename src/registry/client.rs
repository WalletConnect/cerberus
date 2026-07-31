use {
    crate::{
        project::{
            FeaturesResponse, PlanLimits, ProjectData, ProjectDataRequest, ProjectDataResponse,
            ProjectDataWithLimits, ProjectDataWithQuota,
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
    tokio::time::sleep,
    tracing::error,
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

    /// Flexible method to fetch project data with optional additional information.
    ///
    /// This method accepts a `ProjectDataRequest` that specifies which additional
    /// data to include (limits, features, or both) and returns a unified response
    /// structure with optional fields.
    async fn project_data_with(
        &self,
        request: ProjectDataRequest<'_>,
    ) -> RegistryResult<Option<ProjectDataResponse>>;
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
    /// Default is 5 seconds.
    pub timeout: Option<Duration>,

    /// Number of times to retry a failed request.
    ///
    /// Default is 2 retries.
    pub max_retries: usize,

    /// Initial backoff delay before the first retry.
    ///
    /// Default is 100 milliseconds.
    pub initial_backoff: Duration,
}

impl Default for HttpClientConfig {
    fn default() -> Self {
        Self {
            // These defaults are taken from `reqwest` default config.
            pool_idle_timeout: Some(Duration::from_secs(90)),
            pool_max_idle: usize::MAX,
            timeout: Some(Duration::from_secs(5)),
            // 2 retries if the first request fails.
            max_retries: 2,
            // Default retry backoff is 100 milliseconds.
            initial_backoff: Duration::from_millis(100),
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
    max_retries: usize,
    initial_backoff: Duration,
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
            max_retries: config.max_retries,
            initial_backoff: config.initial_backoff,
        })
    }

    async fn get_with_retry(&self, url: Url) -> RegistryResult<reqwest::Response> {
        let mut last_err: Option<reqwest::Error> = None;
        for attempt in 0..=self.max_retries {
            match self.http_client.get(url.clone()).send().await {
                Ok(resp) => return Ok(resp),
                Err(err) => {
                    if attempt < self.max_retries {
                        let capped_attempt = attempt.min(20);
                        let multiplier = 1u64 << capped_attempt;
                        let base_ms = self.initial_backoff.as_millis() as u64;
                        let delay_ms = base_ms.saturating_mul(multiplier);
                        error!("Error fetching URL {url}: {err}. Retrying in {delay_ms}ms.");
                        sleep(Duration::from_millis(delay_ms)).await;
                    } else {
                        error!("Error fetching URL {url}: {err}. No more retries left.");
                    }
                    last_err = Some(err);
                }
            }
        }
        error!(
            "Failed to fetch URL {url} after {0} attempts: {last_err:?}",
            self.max_retries + 1
        );
        Err(RegistryError::Transport(
            last_err.expect("max_retries >= 0 guarantees at least one attempt"),
        ))
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

        let resp = self.get_with_retry(url).await?;

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

        let resp = self.get_with_retry(url).await?;

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

        let resp = self.get_with_retry(url).await?;

        parse_http_response(resp).await
    }

    async fn project_data_with_impl(
        &self,
        request: ProjectDataRequest<'_>,
    ) -> RegistryResult<Option<ProjectDataResponse>> {
        if !is_valid_project_id(request.id) {
            return Ok(None);
        }

        // The three upstream calls are independent and span two hosts — the
        // explorer serves project data, the internal API serves limits and
        // features — so issue them concurrently. Awaiting them in sequence made
        // the wall time of a `project_data_with` call the *sum* of two or three
        // round trips, which is what callers pay on every cache miss.
        //
        // `join!`, deliberately not `try_join!`: `try_join!` short-circuits on
        // the first error, so a transport failure on limits or features would
        // pre-empt a 404 on the base project data and report "registry
        // unavailable" where the sequential code reported "project not found".
        // That is the fail-open bug #33 and #34 fixed. Awaiting all three and
        // then resolving them in the original precedence order below keeps the
        // outcome identical to the sequential version for every input; only the
        // timing changes.
        //
        // The cost of that concurrency: a project that 404s now also issues the
        // limits/features calls, which the sequential version skipped. Unknown
        // project IDs are a negligible share of traffic and callers cache the
        // not-found result, so this trades a rare extra call for a latency win
        // on the common path.
        let (data, limits, features) = tokio::join!(
            self.project_data(request.id),
            async {
                match request.include_limits {
                    true => self.project_limits(request.id).await,
                    false => Ok(None),
                }
            },
            async {
                match request.include_features {
                    true => self.project_features(request.id).await,
                    false => Ok(None),
                }
            },
        );

        // Resolve in the same order the sequential implementation used, so the
        // base project data still decides the outcome first: a missing project
        // (registry returns 404) must surface as `Ok(None)` so callers can tell
        // "project not found" apart from a transient registry error, matching
        // the behaviour of `project_data_with_limits_impl`. Returning an `Err`
        // here causes callers to treat an unknown project as a registry outage
        // and fail open.
        let data = match data? {
            Some(data) => data,
            None => return Ok(None),
        };

        // A missing sub-resource (registry returns 404) must surface as
        // `Ok(None)`, never `Err`, for the same reason as the base project
        // data above and to match `project_data_with_limits_impl`: an `Err`
        // makes callers treat a definitively-absent limit/feature record as a
        // transient registry outage and fail open.
        let limits = match request.include_limits {
            true => match limits? {
                Some(response) => Some(response.plan_limits),
                None => return Ok(None),
            },
            false => None,
        };

        let features = match request.include_features {
            true => match features? {
                Some(response) => Some(response.features),
                None => return Ok(None),
            },
            false => None,
        };

        Ok(Some(ProjectDataResponse {
            data,
            limits,
            features,
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

    async fn project_data_with(
        &self,
        request: ProjectDataRequest<'_>,
    ) -> RegistryResult<Option<ProjectDataResponse>> {
        self.project_data_with_impl(request).await
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
        // Distinguish terminal denials from transient outages so callers can
        // fail closed on the former instead of lumping everything into a
        // generic error that gets classified as a retryable outage.
        StatusCode::FORBIDDEN => Err(RegistryError::Forbidden(format!(
            "status={status} body={:?}",
            resp.text().await
        ))),
        StatusCode::TOO_MANY_REQUESTS => Err(RegistryError::RateLimited(format!(
            "status={status} body={:?}",
            resp.text().await
        ))),
        code if code.is_server_error() => Err(RegistryError::ServerError(format!(
            "status={status} body={:?}",
            resp.text().await
        ))),
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
    async fn project_data_with_returns_none_when_project_not_found() {
        let project_id = "a".repeat(32);

        let mock_server = MockServer::start().await;

        // Registry returns 404 for a well-formed but unregistered project id.
        Mock::given(method(Method::Get))
            .and(path(format!("/internal/project/key/{project_id}")))
            .respond_with(ResponseTemplate::new(404))
            .mount(&mock_server)
            .await;

        let request = crate::project::ProjectDataRequest::new(&project_id).include_limits();

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
        .project_data_with(request)
        .await
        .unwrap();

        // A missing project must be `Ok(None)`, not an `Err`: an `Err` makes
        // callers treat an unknown project as a transient registry outage and
        // fail open instead of rejecting the request.
        assert!(response.is_none());
    }

    #[tokio::test]
    async fn project_data_with_returns_none_when_limits_not_found() {
        let project_id = "a".repeat(32);

        let mock_server = MockServer::start().await;

        // The project itself exists...
        Mock::given(method(Method::Get))
            .and(path(format!("/internal/project/key/{project_id}")))
            .respond_with(ResponseTemplate::new(StatusCode::OK).set_body_json(mock_project_data()))
            .mount(&mock_server)
            .await;

        // ...but its limits record is missing (404).
        Mock::given(method(Method::Get))
            .and(path("/internal/v1/project-limits"))
            .respond_with(ResponseTemplate::new(404))
            .mount(&mock_server)
            .await;

        let request = crate::project::ProjectDataRequest::new(&project_id).include_limits();

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
        .project_data_with(request)
        .await
        .unwrap();

        // A missing limits record must be `Ok(None)`, not an `Err`: an `Err`
        // makes callers treat it as a transient outage and fail open on plan /
        // rate-limit enforcement.
        assert!(response.is_none());
    }

    #[tokio::test]
    async fn project_data_with_returns_none_when_features_not_found() {
        let project_id = "a".repeat(32);

        let mock_server = MockServer::start().await;

        Mock::given(method(Method::Get))
            .and(path(format!("/internal/project/key/{project_id}")))
            .respond_with(ResponseTemplate::new(StatusCode::OK).set_body_json(mock_project_data()))
            .mount(&mock_server)
            .await;

        Mock::given(method(Method::Get))
            .and(path("/appkit/v1/config"))
            .respond_with(ResponseTemplate::new(404))
            .mount(&mock_server)
            .await;

        let request = crate::project::ProjectDataRequest::new(&project_id).include_features();

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
        .project_data_with(request)
        .await
        .unwrap();

        assert!(response.is_none());
    }

    #[tokio::test]
    async fn forbidden_maps_to_forbidden_error() {
        let project_id = "a".repeat(32);

        let mock_server = MockServer::start().await;

        Mock::given(method(Method::Get))
            .and(path(format!("/internal/project/key/{project_id}")))
            .respond_with(ResponseTemplate::new(StatusCode::FORBIDDEN))
            .mount(&mock_server)
            .await;

        let result = RegistryHttpClient::new(mock_server.uri(), "auth", TEST_ORIGIN, "st", "sv")
            .unwrap()
            .project_data(&project_id)
            .await;

        // A 403 is a terminal denial and must be distinguishable from a
        // transient outage so callers can fail closed.
        assert!(matches!(result, Err(RegistryError::Forbidden(_))));
    }

    #[tokio::test]
    async fn server_error_maps_to_server_error() {
        let project_id = "a".repeat(32);

        let mock_server = MockServer::start().await;

        Mock::given(method(Method::Get))
            .and(path(format!("/internal/project/key/{project_id}")))
            .respond_with(ResponseTemplate::new(StatusCode::INTERNAL_SERVER_ERROR))
            .mount(&mock_server)
            .await;

        let result = RegistryHttpClient::new(mock_server.uri(), "auth", TEST_ORIGIN, "st", "sv")
            .unwrap()
            .project_data(&project_id)
            .await;

        assert!(matches!(result, Err(RegistryError::ServerError(_))));
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
    async fn project_data_with_query_includes_limits_and_features() {
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

        let request = crate::project::ProjectDataRequest::new(&project_id)
            .include_limits()
            .include_features();

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
        .project_data_with(request)
        .await
        .unwrap();

        assert!(response.is_some());
        let data = response.unwrap();

        // Check that limits are included
        assert!(data.limits.is_some());
        let limits = data.limits.unwrap();
        assert_eq!(limits.tier, "free");
        assert!(!limits.is_above_rpc_limit);
        assert!(!limits.is_above_mau_limit);

        // Check that features are included
        assert!(data.features.is_some());
        let features = data.features.unwrap();
        assert_eq!(features.len(), 2);
        assert_eq!(features[0].id, "multi_wallet");
        assert_eq!(features[1].id, "social_login");
        assert!(!features[0].is_enabled);
        assert!(features[1].is_enabled);
    }

    #[tokio::test]
    async fn project_data_with_query_limits_only() {
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
                        tier: "premium".to_string(),
                        is_above_rpc_limit: true,
                        is_above_mau_limit: false,
                    },
                }),
            )
            .mount(&mock_server)
            .await;

        let request = crate::project::ProjectDataRequest::new(&project_id).include_limits();

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
        .project_data_with(request)
        .await
        .unwrap();

        assert!(response.is_some());
        let data = response.unwrap();

        // Check that limits are included
        assert!(data.limits.is_some());
        let limits = data.limits.unwrap();
        assert_eq!(limits.tier, "premium");
        assert!(limits.is_above_rpc_limit);
        assert!(!limits.is_above_mau_limit);

        // Check that features are NOT included
        assert!(data.features.is_none());
    }

    #[tokio::test]
    async fn project_data_with_query_features_only() {
        let project_id = "a".repeat(32);
        let mock_server = MockServer::start().await;

        // Mock project data endpoint
        Mock::given(method(Method::Get))
            .and(path(format!("/internal/project/key/{project_id}")))
            .respond_with(ResponseTemplate::new(StatusCode::OK).set_body_json(mock_project_data()))
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

        let request = crate::project::ProjectDataRequest::new(&project_id).include_features();

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
        .project_data_with(request)
        .await
        .unwrap();

        assert!(response.is_some());
        let data = response.unwrap();

        // Check that limits are NOT included
        assert!(data.limits.is_none());

        // Check that features are included
        assert!(data.features.is_some());
        let features = data.features.unwrap();
        assert_eq!(features.len(), 2);
        assert_eq!(features[0].id, "multi_wallet");
        assert_eq!(features[1].id, "social_login");
    }

    #[tokio::test]
    async fn project_data_with_query_basic_only() {
        let project_id = "a".repeat(32);
        let mock_server = MockServer::start().await;

        // Mock project data endpoint
        Mock::given(method(Method::Get))
            .and(path(format!("/internal/project/key/{project_id}")))
            .respond_with(ResponseTemplate::new(StatusCode::OK).set_body_json(mock_project_data()))
            .mount(&mock_server)
            .await;

        let request = crate::project::ProjectDataRequest::new(&project_id);

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
        .project_data_with(request)
        .await
        .unwrap();

        assert!(response.is_some());
        let data = response.unwrap();

        assert_eq!(data.data.name, "");
        assert_eq!(data.data.uuid, "");
        assert_eq!(data.data.creator, "");
        assert!(!data.data.is_enabled);

        // Check that optional fields are NOT included
        assert!(data.limits.is_none());
        assert!(data.features.is_none());
    }

    /// Delay applied to every mocked endpoint in the concurrency test. Large
    /// enough that the sequential/concurrent gap dwarfs scheduling noise.
    const CONCURRENCY_PROBE_DELAY: std::time::Duration = std::time::Duration::from_millis(300);

    /// The three upstream calls must be issued concurrently, not chained.
    ///
    /// With every endpoint delayed by the same amount, a sequential
    /// implementation takes 3x the delay and a concurrent one takes ~1x. The
    /// bound sits at 2x: comfortably above one round trip even on a loaded CI
    /// box, and comfortably below the two round trips the old code needed for
    /// `include_limits` alone.
    #[tokio::test]
    async fn project_data_with_issues_upstream_calls_concurrently() {
        let project_id = "a".repeat(32);
        let mock_server = MockServer::start().await;

        Mock::given(method(Method::Get))
            .and(path(format!("/internal/project/key/{project_id}")))
            .respond_with(
                ResponseTemplate::new(StatusCode::OK)
                    .set_body_json(mock_project_data())
                    .set_delay(CONCURRENCY_PROBE_DELAY),
            )
            .mount(&mock_server)
            .await;

        Mock::given(method(Method::Get))
            .and(path("/internal/v1/project-limits"))
            .respond_with(
                ResponseTemplate::new(StatusCode::OK)
                    .set_body_json(LimitsResponse {
                        plan_limits: crate::project::PlanLimits {
                            tier: "free".to_string(),
                            is_above_rpc_limit: false,
                            is_above_mau_limit: false,
                        },
                    })
                    .set_delay(CONCURRENCY_PROBE_DELAY),
            )
            .mount(&mock_server)
            .await;

        Mock::given(method(Method::Get))
            .and(path("/appkit/v1/config"))
            .respond_with(
                ResponseTemplate::new(StatusCode::OK)
                    .set_body_json(mock_features_response())
                    .set_delay(CONCURRENCY_PROBE_DELAY),
            )
            .mount(&mock_server)
            .await;

        let request = crate::project::ProjectDataRequest::new(&project_id)
            .include_limits()
            .include_features();

        let start = std::time::Instant::now();
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
        .project_data_with(request)
        .await
        .unwrap();
        let elapsed = start.elapsed();

        // All three responses still arrive intact.
        let data = response.expect("project should be found");
        assert!(data.limits.is_some());
        assert!(data.features.is_some());

        assert!(
            elapsed < CONCURRENCY_PROBE_DELAY * 2,
            "three {CONCURRENCY_PROBE_DELAY:?} calls took {elapsed:?}; they are being awaited \
             sequentially rather than concurrently",
        );
    }

    /// A 404 on the base project data means "project not found" and must win
    /// over a transport/server failure on a sub-resource.
    ///
    /// This is why `project_data_with_impl` uses `join!` rather than
    /// `try_join!`: `try_join!` returns the first error it sees, so the failing
    /// limits call would surface as `Err` and callers — which fail open on a
    /// registry error — would admit an unregistered project.
    #[tokio::test]
    async fn project_data_with_reports_not_found_even_when_sub_resource_fails() {
        let project_id = "a".repeat(32);
        let mock_server = MockServer::start().await;

        Mock::given(method(Method::Get))
            .and(path(format!("/internal/project/key/{project_id}")))
            .respond_with(ResponseTemplate::new(404))
            .mount(&mock_server)
            .await;

        // Limits are simultaneously unhealthy.
        Mock::given(method(Method::Get))
            .and(path("/internal/v1/project-limits"))
            .respond_with(ResponseTemplate::new(500))
            .mount(&mock_server)
            .await;

        let request = crate::project::ProjectDataRequest::new(&project_id).include_limits();

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
        .project_data_with(request)
        .await;

        match response {
            Ok(None) => {}
            Ok(Some(_)) => panic!("unregistered project must not resolve to project data"),
            Err(err) => panic!(
                "a 404 on the base project data must surface as Ok(None), not a registry error \
                 that makes callers fail open; got {err:?}"
            ),
        }
    }
}
