use std::time::{Duration, Instant};

use dashmap::DashMap;
use nodejs_semver::Version;
use percent_encoding::{utf8_percent_encode, NON_ALPHANUMERIC};
use serde_json::Value;

const REGISTRY: &str = "https://registry.npmjs.org";
const CACHE_TTL: Duration = Duration::from_secs(60 * 60);

#[derive(Debug, Clone)]
pub struct CachedPackage {
    pub fetched: Instant,
    pub latest: Option<Version>,
    pub homepage: Option<String>,
    pub repository_url: Option<String>,
}

pub struct Registry {
    http: reqwest::Client,
    cache: DashMap<String, CachedPackage>,
}

impl Registry {
    pub fn new() -> Self {
        let http = reqwest::Client::builder()
            .user_agent(concat!("zed-package-json-upgrade/", env!("CARGO_PKG_VERSION")))
            .gzip(true)
            .build()
            .expect("reqwest client");
        Self {
            http,
            cache: DashMap::new(),
        }
    }

    pub async fn fetch(&self, name: &str) -> Option<CachedPackage> {
        if let Some(c) = self.cache.get(name) {
            if c.fetched.elapsed() < CACHE_TTL {
                return Some(c.clone());
            }
        }
        let url = format!("{REGISTRY}/{}", encode_pkg_name(name));
        let resp = self.http.get(&url).send().await.ok()?;
        if !resp.status().is_success() {
            return None;
        }
        let body: Value = resp.json().await.ok()?;
        let latest = body
            .get("dist-tags")
            .and_then(|t| t.get("latest"))
            .and_then(|v| v.as_str())
            .and_then(|s| s.parse::<Version>().ok());
        let homepage = body
            .get("homepage")
            .and_then(|v| v.as_str())
            .map(str::to_string);
        let repository_url = body.get("repository").and_then(|r| match r {
            Value::Object(_) => r.get("url").and_then(|v| v.as_str()).map(normalize_repo_url),
            Value::String(s) => Some(normalize_repo_url(s)),
            _ => None,
        });
        let cached = CachedPackage {
            fetched: Instant::now(),
            latest,
            homepage,
            repository_url,
        };
        self.cache.insert(name.to_string(), cached.clone());
        Some(cached)
    }
}

pub fn changelog_url(repo: &str) -> Option<String> {
    let trimmed = repo.trim_end_matches('/');
    if trimmed.contains("github.com") {
        Some(format!("{trimmed}/releases"))
    } else if trimmed.contains("gitlab.com") {
        Some(format!("{trimmed}/-/releases"))
    } else {
        None
    }
}

fn encode_pkg_name(name: &str) -> String {
    if let Some(rest) = name.strip_prefix('@') {
        format!("@{}", utf8_percent_encode(rest, NON_ALPHANUMERIC))
    } else {
        utf8_percent_encode(name, NON_ALPHANUMERIC).to_string()
    }
}

fn normalize_repo_url(raw: &str) -> String {
    let mut s = raw.trim().to_string();
    if let Some(rest) = s.strip_prefix("git+") {
        s = rest.to_string();
    }
    if let Some(rest) = s.strip_suffix(".git") {
        s = rest.to_string();
    }
    if let Some(rest) = s.strip_prefix("git@github.com:") {
        s = format!("https://github.com/{rest}");
    } else if let Some(rest) = s.strip_prefix("git://") {
        s = format!("https://{rest}");
    } else if let Some(rest) = s.strip_prefix("ssh://git@") {
        s = format!("https://{rest}");
    }
    s
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn encodes_scoped_packages() {
        assert_eq!(encode_pkg_name("@scope/pkg"), "@scope%2Fpkg");
        assert_eq!(encode_pkg_name("react"), "react");
    }

    #[test]
    fn normalizes_git_urls() {
        assert_eq!(
            normalize_repo_url("git+https://github.com/owner/repo.git"),
            "https://github.com/owner/repo"
        );
        assert_eq!(
            normalize_repo_url("git@github.com:owner/repo.git"),
            "https://github.com/owner/repo"
        );
    }

    #[test]
    fn changelog_url_for_known_hosts() {
        assert_eq!(
            changelog_url("https://github.com/o/r"),
            Some("https://github.com/o/r/releases".into())
        );
        assert_eq!(
            changelog_url("https://gitlab.com/o/r"),
            Some("https://gitlab.com/o/r/-/releases".into())
        );
        assert_eq!(changelog_url("https://example.com/x"), None);
    }
}
