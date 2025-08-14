use {
    crate::project::{error::AccessError, Origin},
    serde::{Deserialize, Serialize},
};

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum OriginSource {
    Header,
    BundleId,
    PackageName,
}

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
    pub creator: String,
    pub name: String,
    pub push_url: Option<String>,
    pub keys: Vec<ProjectKey>,
    pub is_enabled: bool,
    pub is_verify_enabled: bool,
    pub is_rate_limited: bool,
    pub allowed_origins: Vec<String>,
    pub verified_domains: Vec<String>,
    pub bundle_ids: Vec<String>,
    pub package_names: Vec<String>,
}

#[derive(Debug, Serialize, Deserialize, PartialEq, Eq, Hash, Clone)]
#[serde(rename_all = "camelCase")]
pub struct ProjectDataWithQuota {
    #[serde(flatten)]
    pub project_data: ProjectData,
    pub quota: Quota,
}
#[derive(Debug, Serialize, Deserialize, PartialEq, Eq, Hash, Clone)]
#[serde(rename_all = "camelCase")]
pub struct Quota {
    pub max: u64,
    pub current: u64,
    pub is_valid: bool,
}

#[derive(Debug, Serialize, Deserialize, PartialEq, Eq, Hash, Clone)]
#[serde(rename_all = "camelCase")]
pub struct ProjectDataWithLimits {
    pub data: ProjectData,
    pub limits: PlanLimits,
}

#[derive(Debug, Serialize, Deserialize, PartialEq, Eq, Hash, Clone)]
#[serde(rename_all = "camelCase")]
pub struct PlanLimits {
    pub tier: String,
    pub is_above_rpc_limit: bool,
    pub is_above_mau_limit: bool,
}

#[derive(Debug, Serialize, Deserialize, PartialEq, Eq, Hash, Clone)]
#[serde(rename_all = "camelCase")]
pub struct Feature {
    pub id: String,
    pub is_enabled: bool,
    pub config: Option<serde_json::Value>,
}

#[derive(Debug, Serialize, Deserialize, PartialEq, Eq, Hash, Clone)]
#[serde(rename_all = "camelCase")]
pub struct FeaturesResponse {
    pub features: Vec<Feature>,
}

#[derive(Debug, Serialize, Deserialize, PartialEq, Eq, Hash, Clone)]
#[serde(rename_all = "camelCase")]
pub struct ProjectDataWithLimitsAndFeatures {
    pub data: ProjectData,
    pub limits: PlanLimits,
    pub features: Vec<Feature>,
}

impl ProjectData {
    pub fn validate_access(
        &self,
        id: &str,
        origin: Option<(&str, OriginSource)>,
    ) -> Result<(), AccessError> {
        // Make sure the project is not disabled globally.
        if !self.is_enabled {
            return Err(AccessError::ProjectInactive);
        }

        // Make sure the key is `is_valid`.
        self.keys
            .iter()
            .position(|key| key.value == id && key.is_valid)
            .ok_or(AccessError::KeyInvalid)?;

        if let Some((origin, source)) = origin {
            let origin = Origin::try_from(origin).map_err(|_| AccessError::OriginNotAllowed)?;

            match source {
                OriginSource::Header => self.check_header(&origin),
                OriginSource::BundleId => self.check_bundle_id(&origin),
                OriginSource::PackageName => self.check_package_name(&origin),
            }
        } else {
            // Origin was not provided. Grant access.
            Ok(())
        }
    }

    #[inline]
    fn check_header(&self, origin: &Origin<'_>) -> Result<(), AccessError> {
        const ALLOWED_LOCAL_HOSTS: [&str; 2] = ["localhost", "127.0.0.1"];

        let host = origin.hostname();

        for entry in ALLOWED_LOCAL_HOSTS {
            if host == entry {
                return Ok(());
            }
        }

        self.check_allow_list(&self.allowed_origins, origin, true)
    }

    #[inline]
    fn check_bundle_id(&self, origin: &Origin<'_>) -> Result<(), AccessError> {
        self.check_allow_list(&self.bundle_ids, origin, false)
    }

    #[inline]
    fn check_package_name(&self, origin: &Origin<'_>) -> Result<(), AccessError> {
        self.check_allow_list(&self.package_names, origin, false)
    }

    fn check_allow_list(
        &self,
        list: &[String],
        origin: &Origin<'_>,
        allow_reverse: bool,
    ) -> Result<(), AccessError> {
        // Allow all origins if the list is empty.
        if list.is_empty() {
            return Ok(());
        }

        for entry in list {
            // Ignore malformed entries.
            let Ok(entry) = Origin::try_from(entry.as_str()) else {
                continue;
            };

            if entry.matches(origin) {
                return Ok(());
            }

            if allow_reverse && entry.matches_rev(origin) {
                return Ok(());
            }
        }

        Err(AccessError::OriginNotAllowed)
    }
}

#[cfg(test)]
mod test {
    use super::*;

    #[test]
    fn origin_validation() {
        let project = ProjectData {
            uuid: "test".to_owned(),
            creator: "test".to_owned(),
            push_url: None,
            name: "test".to_owned(),
            keys: vec![ProjectKey {
                value: "test".to_owned(),
                is_valid: true,
            }],
            verified_domains: vec![],
            is_rate_limited: true,
            is_verify_enabled: false,
            allowed_origins: vec![
                "https://prod.bundle.example.com".to_owned(),
                "https://prod.package.example.com".to_owned(),
                "https://prod.header.example.com".to_owned(),
            ],
            is_enabled: true,
            bundle_ids: vec![
                "com.example.bundle".to_owned(),
                "com.example.bundle.dev".to_owned(),
                "com.example.bundle.staging".to_owned(),
            ],
            package_names: vec![
                "com.example.package".to_owned(),
                "com.example.package.dev".to_owned(),
                "com.example.package.staging".to_owned(),
            ],
        };

        assert!(project
            .validate_access("test", Some(("invalid.host.com", OriginSource::Header)))
            .is_err());
        assert!(project
            .validate_access("test", Some(("invalid.host.com", OriginSource::BundleId)))
            .is_err());
        assert!(project
            .validate_access(
                "test",
                Some(("invalid.host.com", OriginSource::PackageName))
            )
            .is_err());

        assert!(project
            .validate_access(
                "test",
                Some(("prod.header.example.com", OriginSource::Header)),
            )
            .is_ok());
        assert!(project
            .validate_access(
                "test",
                Some(("com.example.header.prod", OriginSource::Header)),
            )
            .is_ok());
        assert!(project
            .validate_access(
                "test",
                Some(("prod.header.example.com", OriginSource::BundleId)),
            )
            .is_err());
        assert!(project
            .validate_access(
                "test",
                Some(("prod.header.example.com", OriginSource::PackageName)),
            )
            .is_err());

        assert!(project
            .validate_access("test", Some(("com.example.bundle", OriginSource::Header)))
            .is_err());
        assert!(project
            .validate_access("test", Some(("com.example.bundle", OriginSource::BundleId)))
            .is_ok());
        assert!(project
            .validate_access(
                "test",
                Some(("com.example.bundle", OriginSource::PackageName))
            )
            .is_err());

        assert!(project
            .validate_access("test", Some(("com.example.package", OriginSource::Header)))
            .is_err());
        assert!(project
            .validate_access(
                "test",
                Some(("com.example.package", OriginSource::BundleId))
            )
            .is_err());
        assert!(project
            .validate_access(
                "test",
                Some(("com.example.package", OriginSource::PackageName)),
            )
            .is_ok());

        let project = ProjectData {
            uuid: "test".to_owned(),
            creator: "test".to_owned(),
            push_url: None,
            name: "test".to_owned(),
            keys: vec![ProjectKey {
                value: "test".to_owned(),
                is_valid: true,
            }],
            verified_domains: vec![],
            is_rate_limited: true,
            is_verify_enabled: false,
            allowed_origins: vec![],
            is_enabled: true,
            bundle_ids: vec![],
            package_names: vec![],
        };

        assert!(project
            .validate_access("test", Some(("invalid.host.com", OriginSource::Header)))
            .is_ok());
        assert!(project
            .validate_access("test", Some(("invalid.host.com", OriginSource::BundleId)))
            .is_ok());
        assert!(project
            .validate_access(
                "test",
                Some(("invalid.host.com", OriginSource::PackageName))
            )
            .is_ok());
    }
}
