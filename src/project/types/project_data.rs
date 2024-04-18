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
    pub quota: Quota,
}

#[derive(Debug, Serialize, Deserialize, PartialEq, Eq, Hash, Clone)]
#[serde(rename_all = "camelCase")]
pub struct Quota {
    pub max: u64,
    pub current: u64,
    pub is_valid: bool,
}

impl ProjectData {
    pub fn validate_access(
        &self,
        id: &str,
        auth_origin: Option<&str>,
        source: OriginSource,
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

        if let Some(auth_origin) = auth_origin {
            let auth_origin =
                Origin::try_from(auth_origin).map_err(|_| AccessError::OriginNotAllowed)?;

            match source {
                OriginSource::Header => self.valiadte_origin_header(&auth_origin, true),
                OriginSource::BundleId => self.validate_origin(&auth_origin, &self.bundle_ids),
                OriginSource::PackageName => {
                    self.validate_origin(&auth_origin, &self.package_names)
                }
            }
        } else {
            // Origin was not provided. Grant access.
            Ok(())
        }
    }

    fn valiadte_origin_header(
        &self,
        auth_origin: &Origin<'_>,
        allow_empty: bool,
    ) -> Result<(), AccessError> {
        if allow_empty && self.allowed_origins.is_empty() {
            // Allow all origins if the list is empty.
            return Ok(());
        }

        const ALLOWED_LOCAL_HOSTS: [&str; 2] = ["localhost", "127.0.0.1"];

        let auth_origin_host = auth_origin.hostname();

        for host in ALLOWED_LOCAL_HOSTS {
            if auth_origin_host == host {
                return Ok(());
            }
        }

        for origin in &self.allowed_origins {
            // Ignore malformed entries.
            let Ok(origin) = Origin::try_from(origin.as_str()) else {
                continue;
            };

            // Support both forward and reverse matching here for backwards compatibility.
            if origin.matches(auth_origin) || origin.matches_rev(auth_origin) {
                return Ok(());
            }
        }

        Err(AccessError::OriginNotAllowed)
    }

    fn validate_origin(
        &self,
        auth_origin: &Origin<'_>,
        allow_list: &[String],
    ) -> Result<(), AccessError> {
        // Allow all origins if the list is empty.
        if allow_list.is_empty() {
            return Ok(());
        }

        for origin in allow_list {
            // Ignore malformed entries.
            let Ok(origin) = Origin::try_from(origin.as_str()) else {
                continue;
            };

            if origin.matches(auth_origin) {
                return Ok(());
            }
        }

        // If we couldn't match, fallback to matching against `allowed_origins` list for
        // backwards compatibility.
        self.valiadte_origin_header(auth_origin, false)
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
                "https://*.example.com".to_owned(),
                "https://prod.bundle.example.com".to_owned(),
                "https://prod.package.example.com".to_owned(),
            ],
            is_enabled: true,
            quota: Quota {
                max: 100000000,
                current: 0,
                is_valid: true,
            },
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
            .validate_access("test", Some("invalid.host.com"), OriginSource::Header)
            .is_err());

        assert!(project
            .validate_access(
                "test",
                Some("prod.bundle.example.com"),
                OriginSource::Header
            )
            .is_ok());

        assert!(project
            .validate_access("test", Some("dev.bundle.example.com"), OriginSource::Header)
            .is_err());

        assert!(project
            .validate_access("test", Some("test.example.com"), OriginSource::Header)
            .is_ok());

        // Allowed because `allowed_origins` is used as a fallback.
        assert!(project
            .validate_access("test", Some("test.example.com"), OriginSource::BundleId)
            .is_ok());

        // Allowed because `allowed_origins` is used as a fallback, and matched in
        // reverse as bundle ID.
        assert!(project
            .validate_access(
                "test",
                Some("com.example.bundle.prod"),
                OriginSource::BundleId
            )
            .is_ok());

        assert!(project
            .validate_access(
                "test",
                Some("com.example.bundle.dev"),
                OriginSource::BundleId
            )
            .is_ok());

        assert!(project
            .validate_access(
                "test",
                Some("com.example.package.dev"),
                OriginSource::BundleId
            )
            .is_err());

        // Allowed because `allowed_origins` is used as a fallback.
        assert!(project
            .validate_access("test", Some("test.example.com"), OriginSource::PackageName)
            .is_ok());

        // Allowed because `allowed_origins` is used as a fallback, and matched in
        // reverse as bundle ID.
        assert!(project
            .validate_access(
                "test",
                Some("com.example.package.prod"),
                OriginSource::PackageName
            )
            .is_ok());

        assert!(project
            .validate_access(
                "test",
                Some("com.example.package.dev"),
                OriginSource::PackageName
            )
            .is_ok());

        assert!(project
            .validate_access(
                "test",
                Some("com.example.bundle.dev"),
                OriginSource::PackageName
            )
            .is_err());

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
            quota: Quota {
                max: 100000000,
                current: 0,
                is_valid: true,
            },
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

        // Allowed because `allowed_origins` list is empty.
        assert!(project
            .validate_access("test", Some("dev.bundle.example.com"), OriginSource::Header)
            .is_ok());

        // Not allowed because `bundle_ids` list is not empty.
        assert!(project
            .validate_access(
                "test",
                Some("test.bundle.example.com"),
                OriginSource::BundleId
            )
            .is_err());

        // Not allowed because `package_names` list is not empty.
        assert!(project
            .validate_access(
                "test",
                Some("test.package.example.com"),
                OriginSource::PackageName
            )
            .is_err());

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
                "https://*.example.com".to_owned(),
                "https://prod.bundle.example.com".to_owned(),
                "https://prod.package.example.com".to_owned(),
            ],
            is_enabled: true,
            quota: Quota {
                max: 100000000,
                current: 0,
                is_valid: true,
            },
            bundle_ids: vec![],
            package_names: vec![],
        };

        assert!(project
            .validate_access("test", Some("dev.bundle.example.com"), OriginSource::Header)
            .is_err());

        // Allowed because `bundle_ids` list is empty.
        assert!(project
            .validate_access(
                "test",
                Some("dev.bundle.example.com"),
                OriginSource::BundleId
            )
            .is_ok());

        // Allowed because `package_names` list is empty.
        assert!(project
            .validate_access(
                "test",
                Some("dev.package.example.com"),
                OriginSource::PackageName
            )
            .is_ok());
    }
}
