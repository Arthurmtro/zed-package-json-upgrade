use crate::audit::{Advisory, AdvisorySeverity, AuditResult};
use crate::document::{current_pinned, DepEntry};
use crate::registry::{changelog_url, CachedPackage};

pub fn render(dep: &DepEntry, pkg: &CachedPackage, audit: Option<&AuditResult>) -> String {
    let mut md = format!("**{}**", dep.name);
    if let Some(latest) = &pkg.latest {
        md.push_str(&format!("  ·  latest `{latest}`"));
    }
    if let Some(license) = &pkg.license {
        md.push_str(&format!("  ·  {license}"));
    }
    md.push_str("\n\n");
    if let Some(desc) = &pkg.description {
        md.push_str(desc);
        md.push_str("\n\n");
    }
    if let Some(home) = &pkg.homepage {
        md.push_str(&format!("[homepage]({home})"));
    }
    if let Some(repo) = pkg.repository_url.as_deref().and_then(changelog_url) {
        md.push_str(&format!("  ·  [changelog]({repo})"));
    }

    let Some(audit) = audit else { return md };
    if audit.advisories.is_empty() {
        return md;
    }
    let header = match current_pinned(&dep.range_str) {
        Some(c) => format!(
            "\n\n---\n\n**Security advisories** ({} for `{c}`)\n",
            audit.advisories.len()
        ),
        None => format!(
            "\n\n---\n\n**Security advisories** ({})\n",
            audit.advisories.len()
        ),
    };
    md.push_str(&header);
    for adv in &audit.advisories {
        md.push_str(&format!("\n- {}", advisory_line(adv)));
    }
    md
}

fn advisory_line(adv: &Advisory) -> String {
    let badge = match adv.severity {
        AdvisorySeverity::Critical => "🔴 critical",
        AdvisorySeverity::High => "🔴 high",
        AdvisorySeverity::Moderate => "🟡 moderate",
        AdvisorySeverity::Low => "🟢 low",
        AdvisorySeverity::Unknown => "⚪ unknown",
    };
    let title = match &adv.url {
        Some(u) => format!("[{}]({u})", adv.id),
        None => adv.id.clone(),
    };
    let fix = adv
        .fixed_in
        .as_ref()
        .map(|f| format!(" — fixed in `{f}`"))
        .unwrap_or_default();
    format!("{badge} · {title} — {}{fix}", adv.summary)
}
