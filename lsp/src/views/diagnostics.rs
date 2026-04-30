use tower_lsp::lsp_types::{
    CodeDescription, Diagnostic, DiagnosticSeverity, NumberOrString, Url,
};

use crate::audit::{Advisory, AdvisorySeverity};
use crate::document::DepEntry;

pub fn invalid_semver(dep: &DepEntry) -> Diagnostic {
    Diagnostic {
        range: dep.value_range,
        severity: Some(DiagnosticSeverity::ERROR),
        code: Some(NumberOrString::String("invalid-semver".into())),
        source: Some("package-json-upgrade".into()),
        message: format!("Invalid semver range: \"{}\"", dep.range_str),
        ..Default::default()
    }
}

pub fn not_found(dep: &DepEntry) -> Diagnostic {
    Diagnostic {
        range: dep.value_range,
        severity: Some(DiagnosticSeverity::WARNING),
        code: Some(NumberOrString::String("not-found".into())),
        source: Some("package-json-upgrade".into()),
        message: format!("Package \"{}\" not found in npm registry", dep.name),
        ..Default::default()
    }
}

pub fn advisory(dep: &DepEntry, adv: &Advisory) -> Diagnostic {
    let severity = match adv.severity {
        AdvisorySeverity::Critical | AdvisorySeverity::High => DiagnosticSeverity::ERROR,
        AdvisorySeverity::Moderate => DiagnosticSeverity::WARNING,
        _ => DiagnosticSeverity::INFORMATION,
    };
    let mut message = format!("[{}] {}", adv.id, adv.summary);
    if let Some(fix) = &adv.fixed_in {
        message.push_str(&format!(" — fixed in {fix}"));
    }
    Diagnostic {
        range: dep.value_range,
        severity: Some(severity),
        code: Some(NumberOrString::String(adv.id.clone())),
        code_description: adv
            .url
            .as_deref()
            .and_then(|u| Url::parse(u).ok())
            .map(|href| CodeDescription { href }),
        source: Some("package-json-upgrade/audit".into()),
        message,
        ..Default::default()
    }
}
