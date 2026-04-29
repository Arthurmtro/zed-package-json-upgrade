use std::collections::HashMap;

use dashmap::DashMap;
use regex::Regex;
use serde_json::Value;
use tokio::sync::RwLock;
use tower_lsp::jsonrpc::Result as LspResult;
use tower_lsp::lsp_types::*;
use tower_lsp::{Client, LanguageServer};

use crate::document::{
    current_pinned, parse_package_json, position_in_range, split_prefix, upgrade_kind, DepEntry,
    DocState,
};
use crate::registry::{changelog_url, Registry};
use crate::settings::Settings;

const CMD_OPEN_URL: &str = "package-json-upgrade.openUrl";
const CMD_UPDATE_ALL: &str = "package-json-upgrade.updateAll";

pub struct Backend {
    client: Client,
    docs: DashMap<Url, DocState>,
    registry: Registry,
    settings: RwLock<Settings>,
}

impl Backend {
    pub fn new(client: Client) -> Self {
        Self {
            client,
            docs: DashMap::new(),
            registry: Registry::new(),
            settings: RwLock::new(Settings::default()),
        }
    }

    async fn refresh(&self, uri: Url) {
        let diags = self.diagnostics_for(&uri).await;
        self.client.publish_diagnostics(uri, diags, None).await;
    }

    async fn diagnostics_for(&self, uri: &Url) -> Vec<Diagnostic> {
        let Some(doc) = self.docs.get(uri).map(|d| d.clone()) else {
            return vec![];
        };
        let settings = self.settings.read().await.clone();
        if !settings.show_updates {
            return vec![];
        }
        let mut out = Vec::new();
        for dep in &doc.deps {
            if is_ignored(&dep.name, &settings) {
                continue;
            }
            let Some(pkg) = self.registry.fetch(&dep.name).await else {
                continue;
            };
            let Some(latest) = &pkg.latest else { continue };
            if dep.range.satisfies(latest) {
                continue;
            }
            if version_ignored(&dep.name, latest, &settings) {
                continue;
            }
            out.push(Diagnostic {
                range: dep.value_range,
                severity: Some(DiagnosticSeverity::HINT),
                code: Some(NumberOrString::String(format!("upgrade:{}", dep.name))),
                source: Some("package-json-upgrade".into()),
                message: format!("{} → {}", dep.range_str, latest),
                ..Default::default()
            });
        }
        out
    }

    async fn inlay_hints_for(&self, uri: &Url) -> Vec<InlayHint> {
        let Some(doc) = self.docs.get(uri).map(|d| d.clone()) else {
            return vec![];
        };
        let settings = self.settings.read().await.clone();
        if !settings.show_updates {
            return vec![];
        }
        let mut hints = Vec::new();
        for dep in &doc.deps {
            if is_ignored(&dep.name, &settings) {
                continue;
            }
            let Some(pkg) = self.registry.fetch(&dep.name).await else {
                continue;
            };
            let Some(latest) = &pkg.latest else { continue };
            if dep.range.satisfies(latest) {
                continue;
            }
            if version_ignored(&dep.name, latest, &settings) {
                continue;
            }
            // Place hint just past the closing quote of the version string.
            let pos = Position {
                line: dep.value_range.end.line,
                character: dep.value_range.end.character.saturating_add(1),
            };
            hints.push(InlayHint {
                position: pos,
                label: InlayHintLabel::String(format!(" → {latest}")),
                kind: Some(InlayHintKind::TYPE),
                text_edits: None,
                tooltip: Some(InlayHintTooltip::String(format!(
                    "{} latest: {}",
                    dep.name, latest
                ))),
                padding_left: Some(true),
                padding_right: None,
                data: None,
            });
        }
        hints
    }

    async fn upgrade_action(
        &self,
        uri: &Url,
        dep: &DepEntry,
        latest: &nodejs_semver::Version,
    ) -> CodeActionOrCommand {
        let current = current_pinned(&dep.range_str);
        let (prefix, _) = split_prefix(&dep.range_str);
        let kind = upgrade_kind(current.as_ref(), latest);
        let title = match kind {
            "major" => format!("Do major upgrade to {latest}"),
            "minor" => format!("Do minor upgrade to {latest}"),
            "patch" => format!("Do patch upgrade to {latest}"),
            _ => format!("Upgrade to {latest}"),
        };
        let new_value = format!("{prefix}{latest}");
        CodeActionOrCommand::CodeAction(CodeAction {
            title,
            kind: Some(CodeActionKind::QUICKFIX),
            edit: Some(workspace_edit(uri, dep.value_range, new_value)),
            is_preferred: Some(true),
            ..Default::default()
        })
    }
}

#[tower_lsp::async_trait]
impl LanguageServer for Backend {
    async fn initialize(&self, _: InitializeParams) -> LspResult<InitializeResult> {
        Ok(InitializeResult {
            server_info: Some(ServerInfo {
                name: "package-json-upgrade".into(),
                version: Some(env!("CARGO_PKG_VERSION").into()),
            }),
            capabilities: ServerCapabilities {
                text_document_sync: Some(TextDocumentSyncCapability::Kind(
                    TextDocumentSyncKind::FULL,
                )),
                code_action_provider: Some(CodeActionProviderCapability::Options(
                    CodeActionOptions {
                        code_action_kinds: Some(vec![
                            CodeActionKind::QUICKFIX,
                            CodeActionKind::new("source.package-json-upgrade.updateAll"),
                        ]),
                        resolve_provider: Some(false),
                        work_done_progress_options: Default::default(),
                    },
                )),
                inlay_hint_provider: Some(OneOf::Left(true)),
                execute_command_provider: Some(ExecuteCommandOptions {
                    commands: vec![CMD_OPEN_URL.into(), CMD_UPDATE_ALL.into()],
                    work_done_progress_options: Default::default(),
                }),
                ..Default::default()
            },
        })
    }

    async fn initialized(&self, _: InitializedParams) {
        let cfg = self
            .client
            .configuration(vec![ConfigurationItem {
                scope_uri: None,
                section: Some("package-json-upgrade".into()),
            }])
            .await
            .ok()
            .and_then(|mut v| v.pop())
            .unwrap_or(Value::Null);
        if !cfg.is_null() {
            if let Ok(parsed) = serde_json::from_value::<Settings>(cfg) {
                *self.settings.write().await = parsed;
            }
        }
    }

    async fn shutdown(&self) -> LspResult<()> {
        Ok(())
    }

    async fn did_open(&self, params: DidOpenTextDocumentParams) {
        let uri = params.text_document.uri.clone();
        if !is_package_json(&uri) {
            return;
        }
        let sections = self.settings.read().await.check_sections.clone();
        self.docs
            .insert(uri.clone(), parse_package_json(&params.text_document.text, &sections));
        self.refresh(uri).await;
    }

    async fn did_change(&self, params: DidChangeTextDocumentParams) {
        let uri = params.text_document.uri.clone();
        if !is_package_json(&uri) {
            return;
        }
        if let Some(change) = params.content_changes.into_iter().last() {
            let sections = self.settings.read().await.check_sections.clone();
            self.docs
                .insert(uri.clone(), parse_package_json(&change.text, &sections));
            self.refresh(uri).await;
        }
    }

    async fn did_close(&self, params: DidCloseTextDocumentParams) {
        let uri = params.text_document.uri;
        self.docs.remove(&uri);
        self.client.publish_diagnostics(uri, vec![], None).await;
    }

    async fn did_change_configuration(&self, params: DidChangeConfigurationParams) {
        if let Ok(parsed) = serde_json::from_value::<Settings>(params.settings) {
            *self.settings.write().await = parsed;
            let uris: Vec<Url> = self.docs.iter().map(|e| e.key().clone()).collect();
            for uri in uris {
                self.refresh(uri).await;
            }
        }
    }

    async fn inlay_hint(&self, params: InlayHintParams) -> LspResult<Option<Vec<InlayHint>>> {
        let uri = params.text_document.uri;
        if !is_package_json(&uri) {
            return Ok(None);
        }
        Ok(Some(self.inlay_hints_for(&uri).await))
    }

    async fn code_action(
        &self,
        params: CodeActionParams,
    ) -> LspResult<Option<CodeActionResponse>> {
        let uri = params.text_document.uri.clone();
        if !is_package_json(&uri) {
            return Ok(None);
        }
        let Some(doc) = self.docs.get(&uri).map(|d| d.clone()) else {
            return Ok(None);
        };
        let cursor = params.range.start;
        let mut actions: Vec<CodeActionOrCommand> = Vec::new();

        for dep in doc.deps.iter() {
            if !position_in_range(cursor, dep.value_range) {
                continue;
            }
            let Some(pkg) = self.registry.fetch(&dep.name).await else {
                continue;
            };
            let Some(latest) = pkg.latest.clone() else {
                continue;
            };
            actions.push(self.upgrade_action(&uri, dep, &latest).await);
            if let Some(home) = pkg.homepage.clone() {
                actions.push(open_url_action("Open homepage", home));
            }
            if let Some(repo) = pkg.repository_url.as_deref().and_then(changelog_url) {
                actions.push(open_url_action("Open changelog", repo));
            }
        }

        actions.push(CodeActionOrCommand::CodeAction(CodeAction {
            title: "Update all dependencies".into(),
            kind: Some(CodeActionKind::new("source.package-json-upgrade.updateAll")),
            command: Some(Command {
                title: "Update all dependencies".into(),
                command: CMD_UPDATE_ALL.into(),
                arguments: Some(vec![Value::String(uri.to_string())]),
            }),
            ..Default::default()
        }));

        Ok(Some(actions))
    }

    async fn execute_command(&self, params: ExecuteCommandParams) -> LspResult<Option<Value>> {
        match params.command.as_str() {
            CMD_OPEN_URL => {
                if let Some(Value::String(url)) = params.arguments.into_iter().next() {
                    if let Ok(parsed) = Url::parse(&url) {
                        let _ = self
                            .client
                            .show_document(ShowDocumentParams {
                                uri: parsed,
                                external: Some(true),
                                take_focus: Some(true),
                                selection: None,
                            })
                            .await;
                    }
                }
                Ok(None)
            }
            CMD_UPDATE_ALL => {
                let Some(Value::String(uri_s)) = params.arguments.into_iter().next() else {
                    return Ok(None);
                };
                let Ok(uri) = Url::parse(&uri_s) else {
                    return Ok(None);
                };
                let Some(doc) = self.docs.get(&uri).map(|d| d.clone()) else {
                    return Ok(None);
                };
                let settings = self.settings.read().await.clone();
                let mut edits = Vec::new();
                for dep in &doc.deps {
                    if is_ignored(&dep.name, &settings) {
                        continue;
                    }
                    let Some(pkg) = self.registry.fetch(&dep.name).await else {
                        continue;
                    };
                    let Some(latest) = pkg.latest else { continue };
                    if dep.range.satisfies(&latest) {
                        continue;
                    }
                    if version_ignored(&dep.name, &latest, &settings) {
                        continue;
                    }
                    let (prefix, _) = split_prefix(&dep.range_str);
                    edits.push(TextEdit {
                        range: dep.value_range,
                        new_text: format!("{prefix}{latest}"),
                    });
                }
                if !edits.is_empty() {
                    let mut changes = HashMap::new();
                    changes.insert(uri.clone(), edits);
                    let _ = self
                        .client
                        .apply_edit(WorkspaceEdit {
                            changes: Some(changes),
                            document_changes: None,
                            change_annotations: None,
                        })
                        .await;
                }
                Ok(None)
            }
            _ => Ok(None),
        }
    }
}

fn open_url_action(title: &str, url: String) -> CodeActionOrCommand {
    CodeActionOrCommand::CodeAction(CodeAction {
        title: title.into(),
        kind: Some(CodeActionKind::EMPTY),
        command: Some(Command {
            title: title.into(),
            command: CMD_OPEN_URL.into(),
            arguments: Some(vec![Value::String(url)]),
        }),
        ..Default::default()
    })
}

fn workspace_edit(uri: &Url, range: Range, new_text: String) -> WorkspaceEdit {
    let mut changes = HashMap::new();
    changes.insert(uri.clone(), vec![TextEdit { range, new_text }]);
    WorkspaceEdit {
        changes: Some(changes),
        document_changes: None,
        change_annotations: None,
    }
}

fn is_package_json(uri: &Url) -> bool {
    uri.path()
        .rsplit('/')
        .next()
        .map(|f| f == "package.json")
        .unwrap_or(false)
}

fn is_ignored(name: &str, settings: &Settings) -> bool {
    settings.ignore_patterns.iter().any(|pat| {
        Regex::new(pat)
            .map(|re| re.is_match(name))
            .unwrap_or(false)
    })
}

fn version_ignored(
    name: &str,
    version: &nodejs_semver::Version,
    settings: &Settings,
) -> bool {
    let Some(rule) = settings.ignore_versions.get(name) else {
        return false;
    };
    rule.parse::<nodejs_semver::Range>()
        .map(|r| r.satisfies(version))
        .unwrap_or(false)
}
