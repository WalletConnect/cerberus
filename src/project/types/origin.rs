use {
    once_cell::sync::Lazy,
    regex::Regex,
    std::{fmt::Display, iter::zip},
};

/// Simplified URL parser regex. Extracts only the scheme (optional), hostname
/// and port (optional). The expression is fully anchored: an optional trailing
/// path (`/...`) is permitted and ignored, but any other unexpected suffix is
/// rejected rather than silently truncated, so a malformed allow-list entry or
/// origin can never parse to an unintended host.
static ORIGIN_PARSER_REGEX: Lazy<Regex> =
    Lazy::new(|| Regex::new(r"^(([^:]+)://)?([^:/]+)(:([\d]+))?(/.*)?$").unwrap());

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum MatchDirection {
    Forward,
    Reverse,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Origin<'a> {
    scheme: Option<&'a str>,
    hostname: &'a str,
    hostname_parts: Vec<&'a str>,
    port: Option<u16>,
}

const WILDCARD: &str = "*";

impl Origin<'_> {
    pub fn matches(&self, other: &Origin) -> bool {
        self.matches_internal(other, MatchDirection::Forward)
    }

    pub fn matches_rev(&self, other: &Origin) -> bool {
        self.matches_internal(other, MatchDirection::Reverse)
    }

    pub fn hostname(&self) -> &str {
        self.hostname
    }

    fn matches_internal(&self, other: &Origin, dir: MatchDirection) -> bool {
        if let (Some(this_scheme), Some(other_scheme)) = (self.scheme, other.scheme) {
            if !this_scheme.eq_ignore_ascii_case(other_scheme) {
                return false;
            }
        }

        if self.port.is_some() && other.port.is_some() && self.port != other.port {
            return false;
        }

        if self.hostname_parts.len() != other.hostname_parts.len() {
            return false;
        }

        match dir {
            MatchDirection::Forward => {
                zip(&self.hostname_parts, &other.hostname_parts).fold(true, match_fold_cb)
            }

            MatchDirection::Reverse => zip(&self.hostname_parts, other.hostname_parts.iter().rev())
                .fold(true, match_fold_cb),
        }
    }
}

#[inline]
fn match_fold_cb(res: bool, (this, other): (&&str, &&str)) -> bool {
    if this == &WILDCARD {
        res
    } else {
        res && this.eq_ignore_ascii_case(other)
    }
}

#[derive(Debug, thiserror::Error, PartialEq, Eq)]
pub enum OriginParseError {
    #[error("invalid origin format")]
    InvalidFormat,
    #[error("invalid port number")]
    InvalidPortNumber,
}

impl<'a> TryFrom<&'a str> for Origin<'a> {
    type Error = OriginParseError;

    fn try_from(s: &'a str) -> Result<Self, Self::Error> {
        let caps = ORIGIN_PARSER_REGEX
            .captures(s)
            .ok_or(OriginParseError::InvalidFormat)?;

        let scheme = caps.get(2).map(|m| m.as_str());

        let hostname = caps
            .get(3)
            .map(|m| m.as_str())
            .ok_or(OriginParseError::InvalidFormat)?;

        // Reject empty labels (leading/trailing/double dots). Otherwise a
        // wildcard allow-list entry such as `*.example.com` would match a
        // hostname like `.example.com`, because the wildcard matches the empty
        // label.
        let hostname_parts: Vec<&str> = hostname.split('.').collect();
        if hostname_parts.iter().any(|part| part.is_empty()) {
            return Err(OriginParseError::InvalidFormat);
        }

        let port = caps
            .get(5)
            .map(|m| m.as_str().parse())
            .transpose()
            .map_err(|_| OriginParseError::InvalidPortNumber)?;

        Ok(Origin {
            scheme,
            hostname,
            hostname_parts,
            port,
        })
    }
}

impl Display for Origin<'_> {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        if let Some(scheme) = &self.scheme {
            write!(f, "{scheme}://")?;
        }

        let mut host_iter = self.hostname_parts.iter();
        let mut host_part = host_iter.next();

        while let Some(part) = host_part {
            f.write_str(part)?;

            host_part = host_iter.next();

            if host_part.is_some() {
                f.write_str(".")?;
            }
        }

        if let Some(port) = self.port {
            write!(f, ":{port}")?;
        }

        Ok(())
    }
}

#[cfg(test)]
mod test {
    use super::{Origin, OriginParseError};

    #[test]
    fn parse_origin() {
        assert_eq!(
            Origin::try_from("domain.name"),
            Ok(Origin {
                scheme: None,
                hostname: "domain.name",
                hostname_parts: vec!["domain", "name"],
                port: None,
            })
        );

        assert_eq!(
            Origin::try_from("domain.name:123"),
            Ok(Origin {
                scheme: None,
                hostname: "domain.name",
                hostname_parts: vec!["domain", "name"],
                port: Some(123),
            })
        );

        assert_eq!(
            Origin::try_from("http://domain.name"),
            Ok(Origin {
                scheme: Some("http"),
                hostname: "domain.name",
                hostname_parts: vec!["domain", "name"],
                port: None,
            })
        );

        assert_eq!(
            Origin::try_from("http://*.domain.name:123"),
            Ok(Origin {
                scheme: Some("http"),
                hostname: "*.domain.name",
                hostname_parts: vec!["*", "domain", "name"],
                port: Some(123),
            })
        );

        assert_eq!(
            Origin::try_from("http://domain.name:99999"),
            Err(OriginParseError::InvalidPortNumber)
        );

        let origin = "http://*.domain.name:123";
        assert_eq!(Origin::try_from(origin).unwrap().to_string(), origin);
    }

    #[test]
    fn origin_matching() {
        let o1 = Origin::try_from("http://a.*.domain.name:123").unwrap();
        let o2 = Origin::try_from("http://a.b.domain.name:123").unwrap();

        assert!(o1.matches(&o2));

        let o1 = Origin::try_from("domain.name").unwrap();
        let o2 = Origin::try_from("domain.name").unwrap();

        assert!(o1.matches(&o2));

        let o1 = Origin::try_from("https://a.b.domain.name").unwrap();
        let o2 = Origin::try_from("http://a.b.domain.name").unwrap();

        assert!(!o1.matches(&o2));

        let o1 = Origin::try_from("domain.name:123").unwrap();
        let o2 = Origin::try_from("domain.name:124").unwrap();

        assert!(!o1.matches(&o2));

        let o1 = Origin::try_from("https://domain.name:123").unwrap();
        let o2 = Origin::try_from("domain.name:124").unwrap();

        assert!(!o1.matches(&o2));

        let o1 = Origin::try_from("https://a.b.domain.name/").unwrap();
        let o2 = Origin::try_from("http://a.b.domain.name").unwrap();

        assert!(!o1.matches(&o2));

        let o1 = Origin::try_from("https://a.b.domain.name/").unwrap();
        let o2 = Origin::try_from("a.b.domain.name").unwrap();

        assert!(o1.matches(&o2));

        let o1 = Origin::try_from("https://react-app.walletconnect.com").unwrap();
        let o2 = Origin::try_from("react-app.walletconnect.com").unwrap();

        assert!(o1.matches(&o2));

        // Allow trailing slash.
        let o1 = Origin::try_from("https://react-app.walletconnect.com/").unwrap();
        let o2 = Origin::try_from("react-app.walletconnect.com").unwrap();

        assert!(o1.matches(&o2));

        // Allow custom schema when it's unspecified.
        let o1 = Origin::try_from("custom-schema://react-app.walletconnect.com/").unwrap();
        let o2 = Origin::try_from("react-app.walletconnect.com").unwrap();

        assert!(o1.matches(&o2));
    }

    #[test]
    fn origin_matching_opt_default() {
        let o1 = Origin::try_from("http://a.*.domain.name:123").unwrap();
        let o2 = Origin::try_from("http://a.b.domain.name:123").unwrap();

        assert!(o1.matches(&o2));

        let o1 = Origin::try_from("domain.name").unwrap();
        let o2 = Origin::try_from("domain.name").unwrap();

        assert!(o1.matches(&o2));

        let o1 = Origin::try_from("https://a.b.domain.name").unwrap();
        let o2 = Origin::try_from("http://a.b.domain.name").unwrap();

        assert!(!o1.matches(&o2));

        let o1 = Origin::try_from("domain.name:123").unwrap();
        let o2 = Origin::try_from("domain.name:124").unwrap();

        assert!(!o1.matches(&o2));

        let o1 = Origin::try_from("https://domain.name:123").unwrap();
        let o2 = Origin::try_from("domain.name:124").unwrap();

        assert!(!o1.matches(&o2));

        let o1 = Origin::try_from("https://a.b.domain.name/").unwrap();
        let o2 = Origin::try_from("http://a.b.domain.name").unwrap();

        assert!(!o1.matches(&o2));

        let o1 = Origin::try_from("https://a.b.domain.name/").unwrap();
        let o2 = Origin::try_from("a.b.domain.name").unwrap();

        assert!(o1.matches(&o2));

        let o1 = Origin::try_from("https://react-app.walletconnect.com").unwrap();
        let o2 = Origin::try_from("react-app.walletconnect.com").unwrap();

        assert!(o1.matches(&o2));

        // Allow trailing slash.
        let o1 = Origin::try_from("https://react-app.walletconnect.com/").unwrap();
        let o2 = Origin::try_from("react-app.walletconnect.com").unwrap();

        assert!(o1.matches(&o2));

        // Allow custom schema when it's unspecified.
        let o1 = Origin::try_from("custom-schema://react-app.walletconnect.com/").unwrap();
        let o2 = Origin::try_from("react-app.walletconnect.com").unwrap();

        assert!(o1.matches(&o2));
    }

    #[test]
    fn origin_matching_opt_scheme() {
        let o1 = Origin::try_from("http://a.*.domain.name:123").unwrap();
        let o2 = Origin::try_from("http://a.b.domain.name:123").unwrap();
        assert!(o1.matches(&o2));

        let o1 = Origin::try_from("http://a.*.domain.name:123").unwrap();
        let o2 = Origin::try_from("http://a.b.domain.name:123").unwrap();
        assert!(o1.matches(&o2));

        let o1 = Origin::try_from("http://a.b.domain.name:123").unwrap();
        let o2 = Origin::try_from("https://a.b.domain.name:123").unwrap();
        assert!(!o1.matches(&o2));

        let o1 = Origin::try_from("a.b.domain.name:123").unwrap();
        let o2 = Origin::try_from("http://a.b.domain.name:123").unwrap();
        assert!(o1.matches(&o2));

        let o1 = Origin::try_from("http://a.b.domain.name:123").unwrap();
        let o2 = Origin::try_from("a.b.domain.name:123").unwrap();
        assert!(o1.matches(&o2));

        let o1 = Origin::try_from("http://a.*.domain.name:123").unwrap();
        let o2 = Origin::try_from("http://a.b.domain.name:123").unwrap();
        assert!(o1.matches(&o2));

        let o1 = Origin::try_from("http://a.b.domain.name:123").unwrap();
        let o2 = Origin::try_from("https://a.b.domain.name:123").unwrap();
        assert!(!o1.matches(&o2));

        let o1 = Origin::try_from("a.b.domain.name:123").unwrap();
        let o2 = Origin::try_from("http://a.b.domain.name:123").unwrap();
        assert!(o1.matches(&o2));

        let o1 = Origin::try_from("http://a.b.domain.name:123").unwrap();
        let o2 = Origin::try_from("a.b.domain.name:123").unwrap();
        assert!(o1.matches(&o2));
    }

    #[test]
    fn origin_matching_opt_port() {
        let o1 = Origin::try_from("http://a.*.domain.name:123").unwrap();
        let o2 = Origin::try_from("http://a.b.domain.name:123").unwrap();
        assert!(o1.matches(&o2));

        let o1 = Origin::try_from("https://a.*.domain.name:123").unwrap();
        let o2 = Origin::try_from("https://a.b.domain.name:123").unwrap();
        assert!(o1.matches(&o2));

        let o1 = Origin::try_from("http://a.b.domain.name:123").unwrap();
        let o2 = Origin::try_from("http://a.b.domain.name:456").unwrap();
        assert!(!o1.matches(&o2));

        let o1 = Origin::try_from("http://a.b.domain.name").unwrap();
        let o2 = Origin::try_from("http://a.b.domain.name:123").unwrap();
        assert!(o1.matches(&o2));

        let o1 = Origin::try_from("http://a.b.domain.name:123").unwrap();
        let o2 = Origin::try_from("http://a.b.domain.name").unwrap();
        assert!(o1.matches(&o2));

        let o1 = Origin::try_from("http://a.*.domain.name:123").unwrap();
        let o2 = Origin::try_from("http://a.b.domain.name:123").unwrap();
        assert!(o1.matches(&o2));

        let o1 = Origin::try_from("https://a.*.domain.name:123").unwrap();
        let o2 = Origin::try_from("https://a.b.domain.name:123").unwrap();
        assert!(o1.matches(&o2));

        let o1 = Origin::try_from("http://a.b.domain.name:123").unwrap();
        let o2 = Origin::try_from("http://a.b.domain.name:456").unwrap();
        assert!(!o1.matches(&o2));

        let o1 = Origin::try_from("http://a.b.domain.name").unwrap();
        let o2 = Origin::try_from("http://a.b.domain.name:123").unwrap();
        assert!(o1.matches(&o2));

        let o1 = Origin::try_from("http://a.b.domain.name:123").unwrap();
        let o2 = Origin::try_from("http://a.b.domain.name").unwrap();
        assert!(o1.matches(&o2));
    }

    #[test]
    fn origin_matching_opt_bundle_id() {
        let o1 = Origin::try_from("http://a.*.domain.name:123").unwrap();
        let o2 = Origin::try_from("http://a.b.domain.name:123").unwrap();

        assert!(o1.matches(&o2));

        let o1 = Origin::try_from("domain.name").unwrap();
        let o2 = Origin::try_from("domain.name").unwrap();

        assert!(o1.matches(&o2));

        let o1 = Origin::try_from("https://a.b.domain.name").unwrap();
        let o2 = Origin::try_from("http://a.b.domain.name").unwrap();

        assert!(!o1.matches(&o2));

        let o1 = Origin::try_from("domain.name:123").unwrap();
        let o2 = Origin::try_from("domain.name:124").unwrap();

        assert!(!o1.matches(&o2));

        let o1 = Origin::try_from("https://domain.name:123").unwrap();
        let o2 = Origin::try_from("domain.name:124").unwrap();

        assert!(!o1.matches(&o2));

        let o1 = Origin::try_from("https://a.b.domain.name/").unwrap();
        let o2 = Origin::try_from("http://a.b.domain.name").unwrap();

        assert!(!o1.matches(&o2));

        let o1 = Origin::try_from("https://a.b.domain.name/").unwrap();
        let o2 = Origin::try_from("a.b.domain.name").unwrap();

        assert!(o1.matches(&o2));

        let o1 = Origin::try_from("https://react-app.walletconnect.com").unwrap();
        let o2 = Origin::try_from("react-app.walletconnect.com").unwrap();

        assert!(o1.matches(&o2));

        // Allow trailing slash.
        let o1 = Origin::try_from("https://react-app.walletconnect.com/").unwrap();
        let o2 = Origin::try_from("react-app.walletconnect.com").unwrap();

        assert!(o1.matches(&o2));

        // Allow custom schema when it's unspecified.
        let o1 = Origin::try_from("custom-schema://react-app.walletconnect.com/").unwrap();
        let o2 = Origin::try_from("react-app.walletconnect.com").unwrap();

        assert!(o1.matches(&o2));

        let o1 = Origin::try_from("http://a.b.domain.name").unwrap();
        let o2 = Origin::try_from("http://a.b.domain.name").unwrap();

        assert!(o1.matches(&o2));

        let o1 = Origin::try_from("http://a.b.domain.name").unwrap();
        let o2 = Origin::try_from("http://name.domain.b.a").unwrap();

        assert!(o1.matches_rev(&o2));

        let o1 = Origin::try_from("http://a.b.domain.name").unwrap();
        let o2 = Origin::try_from("name.domain.b.a").unwrap();

        assert!(o1.matches_rev(&o2));
    }

    #[test]
    fn rejects_empty_labels() {
        // Leading, trailing and double dots produce empty labels and must be
        // rejected so a `*` wildcard entry cannot match them.
        assert_eq!(
            Origin::try_from(".example.com"),
            Err(OriginParseError::InvalidFormat)
        );
        assert_eq!(
            Origin::try_from("example.com."),
            Err(OriginParseError::InvalidFormat)
        );
        assert_eq!(
            Origin::try_from("a..b.com"),
            Err(OriginParseError::InvalidFormat)
        );

        // A wildcard entry must not match an empty-label hostname.
        let entry = Origin::try_from("*.example.com").unwrap();
        assert!(Origin::try_from(".example.com").is_err());
        // And a real subdomain still matches.
        assert!(entry.matches(&Origin::try_from("evil.example.com").unwrap()));
    }

    #[test]
    fn rejects_unexpected_trailing_garbage() {
        // The regex is fully anchored: a malformed suffix is rejected rather
        // than silently truncated to a host.
        assert_eq!(
            Origin::try_from("domain.name:notaport"),
            Err(OriginParseError::InvalidFormat)
        );

        // A trailing path is still accepted and ignored.
        assert_eq!(
            Origin::try_from("https://a.b.domain.name/some/path")
                .unwrap()
                .hostname(),
            "a.b.domain.name"
        );
    }

    #[test]
    fn matching_is_case_insensitive() {
        let entry = Origin::try_from("https://App.Example.COM").unwrap();
        let origin = Origin::try_from("https://app.example.com").unwrap();
        assert!(entry.matches(&origin));

        // Scheme comparison is case-insensitive too.
        let entry = Origin::try_from("HTTPS://app.example.com").unwrap();
        let origin = Origin::try_from("https://app.example.com").unwrap();
        assert!(entry.matches(&origin));
    }
}
