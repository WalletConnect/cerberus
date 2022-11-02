mod client;
mod error;

use serde::{Deserialize, Serialize};
pub use {client::*, error::*};

#[derive(Debug, Serialize, Deserialize, PartialEq, Eq, Hash, Clone)]
#[serde(rename_all = "camelCase")]
pub struct ProjectKey {
    pub value: String,
    pub is_valid: bool,
}
