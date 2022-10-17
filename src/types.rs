use serde::{Deserialize, Serialize};

#[derive(Debug, Serialize, Deserialize, PartialEq, Eq, Hash, Clone)]
#[serde(rename_all = "camelCase")]
pub struct ProjectKey {
    pub value: String,
    pub is_valid: bool,
}

#[derive(Debug, Serialize, Deserialize, PartialEq, Eq, Hash, Clone)]
#[serde(rename_all = "camelCase")]
pub struct ProjectData {
    pub uuid: String,
    pub name: String,
    pub push_url: Option<String>,
    pub keys: Vec<ProjectKey>,
    pub is_enabled: bool,
    pub allowed_origins: Vec<String>,
}
