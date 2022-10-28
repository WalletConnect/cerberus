use common::storage::error::StorageError;
use serde::{Deserialize, Serialize};
use thiserror::Error as ThisError;

use crate::registry::RegistryError;

#[derive(Debug, ThisError)]
pub enum ProjectError {
    #[error("registry error: {0}")]
    Registry(#[from] RegistryError),

    #[error("cache error: {0}")]
    Cache(#[from] StorageError),

    #[error("project data error: {0}")]
    ProjectData(#[from] ProjectDataError),

    #[error("project not found")]
    NotFound,

    #[error("registry configuration error")]
    RegistryConfigError,
}

#[derive(Debug, Clone, Serialize, Deserialize, ThisError)]
pub enum ProjectDataError {
    #[error("project not found")]
    NotFound,

    #[error("registry configuration error")]
    RegistryConfigError,
}
