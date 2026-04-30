use nodejs_semver::Version;

use crate::audit::AuditResult;
use crate::document::{pick_tier_target, DepEntry};
use crate::registry::CachedPackage;
use crate::settings::Settings;
use crate::views::util::version_ignored;

pub fn upgrade_label(
    dep: &DepEntry,
    pkg: &CachedPackage,
    current: &Version,
    settings: &Settings,
) -> Option<String> {
    let mut emitted = Vec::new();
    let mut parts = Vec::new();
    for (tier, marker) in [("patch", "🟢"), ("minor", "🟡"), ("major", "🔴")] {
        let Some(target) = pick_tier_target(&pkg.versions, current, tier) else {
            continue;
        };
        if &target <= current || version_ignored(&dep.name, &target, settings) {
            continue;
        }
        let target_str = target.to_string();
        if emitted.contains(&target_str) {
            continue;
        }
        emitted.push(target_str.clone());
        parts.push(format!("{marker} {target_str}"));
    }
    if parts.is_empty() {
        None
    } else {
        Some(format!(" {}", parts.join(" · ")))
    }
}

pub fn advisory_badge(audit: &AuditResult) -> Option<String> {
    if audit.advisories.is_empty() {
        return None;
    }
    let (crit, high, mod_, low) = audit.counts();
    let total = crit + high + mod_ + low;
    let badge = if crit > 0 {
        "🚨"
    } else if high > 0 {
        "‼"
    } else if mod_ > 0 {
        "⚠"
    } else {
        "ℹ"
    };
    let suffix = if total == 1 { "vuln" } else { "vulns" };
    Some(format!(" {badge} {total} {suffix}"))
}

pub fn audit_tooltip(name: &str, audit: Option<&AuditResult>) -> String {
    let Some(audit) = audit else {
        return name.to_string();
    };
    if audit.advisories.is_empty() {
        return name.to_string();
    }
    let suffix = if audit.advisories.len() == 1 { "y" } else { "ies" };
    let mut s = format!("{name} — {} advisor{suffix}", audit.advisories.len());
    for adv in audit.advisories.iter().take(5) {
        s.push_str(&format!("\n• [{}] {}", adv.id, adv.summary));
    }
    if audit.advisories.len() > 5 {
        s.push_str(&format!("\n… +{} more", audit.advisories.len() - 5));
    }
    s
}
