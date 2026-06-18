use {reqwest::header::InvalidHeaderValue, thiserror::Error as ThisError};

#[derive(ThisError, Debug)]
pub enum RegistryError {
    #[error("transport error: {0}")]
    Transport(reqwest::Error),

    #[error("invalid config: {0}")]
    Config(&'static str),

    #[error("origin parse: {0}")]
    OriginParse(InvalidHeaderValue),

    #[error("json parse error: {0}")]
    ResponseJsonParse(reqwest::Error),

    #[error("invalid response: {0}")]
    Response(String),

    /// The registry definitively rejected the request (HTTP 403). This is a
    /// terminal denial, not a transient outage: callers must fail closed.
    #[error("forbidden: {0}")]
    Forbidden(String),

    /// The registry rate-limited the request (HTTP 429).
    #[error("rate limited: {0}")]
    RateLimited(String),

    /// The registry failed to serve the request (HTTP 5xx). This is a
    /// transient, server-side error and is safe to retry.
    #[error("server error: {0}")]
    ServerError(String),

    #[error("building URL: {0}")]
    UrlBuild(url::ParseError),

    #[error("BaseUrlIntoUrl: {0}")]
    BaseUrlIntoUrl(reqwest::Error),

    #[error("building client: {0}")]
    BuildClient(reqwest::Error),
}
