use std::collections::HashMap;

use nodejs_semver::Version;
use serde_json::Value;
use tower_lsp::lsp_types::{
    CodeAction, CodeActionKind, CodeActionOrCommand, Command, Range as LspRange, TextEdit, Url,
    WorkspaceEdit,
};

use crate::audit::AuditResult;
use crate::commands;
use crate::document::{current_pinned, pick_tier_target, split_prefix, upgrade_kind, DepEntry};

const TIERS: [&str; 3] = ["patch", "minor", "major"];

pub fn upgrade_actions(
    uri: &Url,
    dep: &DepEntry,
    versions: &[Version],
    latest: &Version,
) -> Vec<CodeActionOrCommand> {
    let Some(current) = current_pinned(&dep.range_str) else {
        return Vec::new();
    };
    let preferred = upgrade_kind(Some(&current), latest);
    let (prefix, _) = split_prefix(&dep.range_str);

    let mut emitted = Vec::new();
    let mut actions = Vec::new();
    for tier in TIERS {
        let Some(target) = pick_tier_target(versions, &current, tier) else {
            continue;
        };
        if target <= current {
            continue;
        }
        let target_str = target.to_string();
        if emitted.contains(&target_str) {
            continue;
        }
        emitted.push(target_str.clone());
        actions.push(upgrade_action(uri, dep, prefix, tier, &target_str, tier == preferred));
    }
    actions
}

fn upgrade_action(
    uri: &Url,
    dep: &DepEntry,
    prefix: &str,
    tier: &str,
    target: &str,
    preferred: bool,
) -> CodeActionOrCommand {
    let title = match tier {
        "patch" => format!("Patch update to {target}"),
        "minor" => format!("Minor update to {target}"),
        "major" => format!("Major update to {target}"),
        _ => format!("Update to {target}"),
    };
    CodeActionOrCommand::CodeAction(CodeAction {
        title,
        kind: Some(CodeActionKind::QUICKFIX),
        edit: Some(workspace_edit(uri, dep.value_range, format!("{prefix}{target}"))),
        is_preferred: Some(preferred),
        ..Default::default()
    })
}

pub fn safe_upgrade_action(
    uri: &Url,
    dep: &DepEntry,
    audit: &AuditResult,
) -> Option<CodeActionOrCommand> {
    let current = current_pinned(&dep.range_str)?;
    let target = audit.safe_target()?;
    if target <= current {
        return None;
    }
    let (prefix, _) = split_prefix(&dep.range_str);
    Some(CodeActionOrCommand::CodeAction(CodeAction {
        title: format!("Update to first non-vulnerable version ({target})"),
        kind: Some(CodeActionKind::QUICKFIX),
        edit: Some(workspace_edit(
            uri,
            dep.value_range,
            format!("{prefix}{target}"),
        )),
        is_preferred: Some(true),
        ..Default::default()
    }))
}

pub fn bulk_update_actions(uri: &Url) -> Vec<CodeActionOrCommand> {
    TIERS
        .iter()
        .map(|tier| {
            let title = match *tier {
                "patch" => "Update all dependencies (patch)",
                "minor" => "Update all dependencies (minor)",
                _ => "Update all dependencies (major)",
            };
            CodeActionOrCommand::CodeAction(CodeAction {
                title: title.into(),
                kind: Some(CodeActionKind::new("source.package-json-upgrade.updateAll")),
                command: Some(Command {
                    title: title.into(),
                    command: commands::UPDATE_ALL.into(),
                    arguments: Some(vec![
                        Value::String(uri.to_string()),
                        Value::String((*tier).into()),
                    ]),
                }),
                ..Default::default()
            })
        })
        .collect()
}

pub fn open_url_action(title: &str, url: String) -> CodeActionOrCommand {
    CodeActionOrCommand::CodeAction(CodeAction {
        title: title.into(),
        kind: Some(CodeActionKind::EMPTY),
        command: Some(Command {
            title: title.into(),
            command: commands::OPEN_URL.into(),
            arguments: Some(vec![Value::String(url)]),
        }),
        ..Default::default()
    })
}

pub fn workspace_edit(uri: &Url, range: LspRange, new_text: String) -> WorkspaceEdit {
    let mut changes = HashMap::new();
    changes.insert(uri.clone(), vec![TextEdit { range, new_text }]);
    WorkspaceEdit {
        changes: Some(changes),
        document_changes: None,
        change_annotations: None,
    }
}
