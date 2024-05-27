use thiserror::Error as ThisError;

#[derive(ThisError, Debug)]
pub enum RegistryError {
    #[error("transport error: {0}")]
    Transport(reqwest::Error),

    #[error("invalid config: {0}")]
    Config(&'static str),

    #[error("json parse error: {0}")]
    ResponseJsonParse(reqwest::Error),

    #[error("invalid response: {0}")]
    Response(String),

    #[error("building URL: {0}")]
    UrlBuild(url::ParseError),

    #[error("BaseUrlIntoUrl: {0}")]
    BaseUrlIntoUrl(reqwest::Error),

    #[error("building client: {0}")]
    BuildClient(reqwest::Error),
}
