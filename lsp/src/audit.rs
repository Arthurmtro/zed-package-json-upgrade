use std::sync::Arc;
use std::time::{Duration, Instant};

use dashmap::DashMap;
use nodejs_semver::Version;
use serde_json::Value;
use tokio::sync::Semaphore;

const OSV_QUERY: &str = "https://api.osv.dev/v1/query";
const CACHE_TTL: Duration = Duration::from_secs(6 * 60 * 60);
const CONCURRENT_QUERIES: usize = 8;

#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord)]
pub enum AdvisorySeverity {
    Low,
    Moderate,
    High,
    Critical,
    Unknown,
}

#[derive(Debug, Clone)]
pub struct Advisory {
    pub id: String,
    pub summary: String,
    pub severity: AdvisorySeverity,
    pub fixed_in: Option<Version>,
    pub url: Option<String>,
}

#[derive(Debug, Clone)]
pub struct AuditResult {
    pub advisories: Vec<Advisory>,
    fetched: Instant,
}

impl AuditResult {
    fn fresh() -> Self {
        Self {
            advisories: Vec::new(),
            fetched: Instant::now(),
        }
    }

    pub fn counts(&self) -> (usize, usize, usize, usize) {
        let mut c = (0, 0, 0, 0); // (critical, high, moderate, low)
        for adv in &self.advisories {
            match adv.severity {
                AdvisorySeverity::Critical => c.0 += 1,
                AdvisorySeverity::High => c.1 += 1,
                AdvisorySeverity::Moderate => c.2 += 1,
                AdvisorySeverity::Low | AdvisorySeverity::Unknown => c.3 += 1,
            }
        }
        c
    }

    /// Lowest version that resolves all advisories (max of every advisory's
    /// `fixed_in`). `None` when no advisory has a documented fix yet.
    pub fn safe_target(&self) -> Option<Version> {
        self.advisories
            .iter()
            .filter_map(|a| a.fixed_in.clone())
            .max()
    }
}

pub struct Auditor {
    http: reqwest::Client,
    cache: DashMap<String, AuditResult>,
    permits: Arc<Semaphore>,
}

impl Auditor {
    pub fn new() -> Self {
        let http = reqwest::Client::builder()
            .user_agent(concat!("zed-package-json-upgrade/", env!("CARGO_PKG_VERSION")))
            .gzip(true)
            .build()
            .expect("reqwest client");
        Self {
            http,
            cache: DashMap::new(),
            permits: Arc::new(Semaphore::new(CONCURRENT_QUERIES)),
        }
    }

    pub async fn audit(&self, name: &str, version: &Version) -> Option<AuditResult> {
        let key = format!("{name}@{version}");
        if let Some(r) = self.cache.get(&key) {
            if r.fetched.elapsed() < CACHE_TTL {
                return Some(r.clone());
            }
        }
        let _permit = self.permits.acquire().await.ok()?;
        if let Some(r) = self.cache.get(&key) {
            if r.fetched.elapsed() < CACHE_TTL {
                return Some(r.clone());
            }
        }

        let body = serde_json::json!({
            "package": { "name": name, "ecosystem": "npm" },
            "version": version.to_string(),
        });
        let resp = self.http.post(OSV_QUERY).json(&body).send().await.ok()?;
        if !resp.status().is_success() {
            return None;
        }
        let json: Value = resp.json().await.ok()?;
        let mut result = AuditResult::fresh();
        if let Some(vulns) = json.get("vulns").and_then(Value::as_array) {
            for vuln in vulns {
                if let Some(adv) = parse_advisory(vuln, name) {
                    result.advisories.push(adv);
                }
            }
        }
        self.cache.insert(key, result.clone());
        Some(result)
    }
}

fn parse_advisory(vuln: &Value, package: &str) -> Option<Advisory> {
    let id = vuln.get("id").and_then(Value::as_str)?.to_string();
    let summary = vuln
        .get("summary")
        .and_then(Value::as_str)
        .unwrap_or("Vulnerability")
        .to_string();

    let severity = parse_severity(vuln);
    let fixed_in = parse_fixed_in(vuln, package);
    let url = vuln
        .get("references")
        .and_then(Value::as_array)
        .and_then(|refs| {
            refs.iter()
                .find(|r| {
                    r.get("type")
                        .and_then(Value::as_str)
                        .map(|t| t == "ADVISORY" || t == "WEB")
                        .unwrap_or(false)
                })
                .or_else(|| refs.first())
        })
        .and_then(|r| r.get("url"))
        .and_then(Value::as_str)
        .map(str::to_string);

    Some(Advisory {
        id,
        summary,
        severity,
        fixed_in,
        url,
    })
}

fn parse_severity(vuln: &Value) -> AdvisorySeverity {
    if let Some(s) = vuln
        .get("database_specific")
        .and_then(|d| d.get("severity"))
        .and_then(Value::as_str)
    {
        return match s.to_ascii_uppercase().as_str() {
            "CRITICAL" => AdvisorySeverity::Critical,
            "HIGH" => AdvisorySeverity::High,
            "MODERATE" | "MEDIUM" => AdvisorySeverity::Moderate,
            "LOW" => AdvisorySeverity::Low,
            _ => AdvisorySeverity::Unknown,
        };
    }
    if let Some(score) = vuln
        .get("severity")
        .and_then(Value::as_array)
        .and_then(|arr| arr.iter().find(|e| matches_cvss_type(e)))
        .and_then(|e| e.get("score").and_then(Value::as_str))
        .and_then(cvss_base_score)
    {
        return match score {
            s if s >= 9.0 => AdvisorySeverity::Critical,
            s if s >= 7.0 => AdvisorySeverity::High,
            s if s >= 4.0 => AdvisorySeverity::Moderate,
            s if s > 0.0 => AdvisorySeverity::Low,
            _ => AdvisorySeverity::Unknown,
        };
    }
    AdvisorySeverity::Unknown
}

fn matches_cvss_type(e: &Value) -> bool {
    e.get("type")
        .and_then(Value::as_str)
        .map(|t| t.starts_with("CVSS"))
        .unwrap_or(false)
}

/// Extract numeric base score from a CVSS string. Supports two shapes:
/// the bare numeric score ("7.5") and the embedded base metric in a vector
/// ("CVSS:3.1/AV:N/AC:L/.../B:7.5"). Returns `None` when no recognizable
/// numeric score is present.
fn cvss_base_score(vector: &str) -> Option<f64> {
    if let Ok(score) = vector.parse::<f64>() {
        return Some(score);
    }
    for part in vector.split('/') {
        if let Some(rest) = part.strip_prefix("B:") {
            if let Ok(s) = rest.parse::<f64>() {
                return Some(s);
            }
        }
    }
    None
}

fn parse_fixed_in(vuln: &Value, package: &str) -> Option<Version> {
    let affected = vuln.get("affected").and_then(Value::as_array)?;
    let mut highest_fix: Option<Version> = None;
    for entry in affected {
        let name_match = entry
            .get("package")
            .and_then(|p| p.get("name"))
            .and_then(Value::as_str)
            .map(|n| n == package)
            .unwrap_or(false);
        if !name_match {
            continue;
        }
        let Some(ranges) = entry.get("ranges").and_then(Value::as_array) else {
            continue;
        };
        for range in ranges {
            let Some(events) = range.get("events").and_then(Value::as_array) else {
                continue;
            };
            for ev in events {
                if let Some(fixed) = ev.get("fixed").and_then(Value::as_str) {
                    if let Ok(parsed) = fixed.parse::<Version>() {
                        if highest_fix.as_ref().is_none_or(|h| parsed > *h) {
                            highest_fix = Some(parsed);
                        }
                    }
                }
            }
        }
    }
    highest_fix
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_github_severity() {
        let v = serde_json::json!({
            "id": "GHSA-x",
            "summary": "boom",
            "database_specific": { "severity": "HIGH" },
            "affected": []
        });
        let adv = parse_advisory(&v, "lodash").unwrap();
        assert_eq!(adv.severity, AdvisorySeverity::High);
    }

    #[test]
    fn parses_cvss_score_vector() {
        let v = serde_json::json!({
            "id": "GHSA-y",
            "summary": "x",
            "severity": [{ "type": "CVSS_V3", "score": "9.8" }],
            "affected": []
        });
        let adv = parse_advisory(&v, "lodash").unwrap();
        assert_eq!(adv.severity, AdvisorySeverity::Critical);
    }

    #[test]
    fn picks_highest_fix_across_ranges() {
        let v = serde_json::json!({
            "id": "GHSA-z",
            "summary": "x",
            "affected": [
                { "package": { "name": "lodash", "ecosystem": "npm" },
                  "ranges": [
                    { "type": "SEMVER", "events": [{"introduced": "0"}, {"fixed": "4.17.20"}] },
                    { "type": "SEMVER", "events": [{"introduced": "0"}, {"fixed": "4.17.21"}] }
                  ]
                }
            ]
        });
        let adv = parse_advisory(&v, "lodash").unwrap();
        assert_eq!(adv.fixed_in.unwrap().to_string(), "4.17.21");
    }

    #[test]
    fn safe_target_is_max_fix_across_advisories() {
        let mut r = AuditResult::fresh();
        r.advisories.push(Advisory {
            id: "a".into(),
            summary: "".into(),
            severity: AdvisorySeverity::High,
            fixed_in: Some("4.17.20".parse().unwrap()),
            url: None,
        });
        r.advisories.push(Advisory {
            id: "b".into(),
            summary: "".into(),
            severity: AdvisorySeverity::High,
            fixed_in: Some("4.17.21".parse().unwrap()),
            url: None,
        });
        assert_eq!(r.safe_target().unwrap().to_string(), "4.17.21");
    }
}
