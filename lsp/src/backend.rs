use dashmap::DashMap;
use futures::future::join_all;
use nodejs_semver::Version;
use serde_json::Value;
use tokio::sync::RwLock;
use tower_lsp::jsonrpc::Result as LspResult;
use tower_lsp::lsp_types::*;
use tower_lsp::{Client, LanguageServer};

use crate::audit::{AuditResult, Auditor};
use crate::commands;
use crate::document::{
    current_pinned, parse_package_json, pick_tier_target, position_in_range, split_prefix,
    DepEntry, DocState,
};
use crate::registry::{CachedPackage, PackageStatus, Registry};
use crate::settings::Settings;
use crate::views::{actions, diagnostics, hover, inlay, util};

const COMPLETION_LIMIT: usize = 60;

pub struct Backend {
    client: Client,
    docs: DashMap<Url, DocState>,
    registry: Registry,
    auditor: Auditor,
    settings: RwLock<Settings>,
}

impl Backend {
    pub fn new(client: Client) -> Self {
        Self {
            client,
            docs: DashMap::new(),
            registry: Registry::new(),
            auditor: Auditor::new(),
            settings: RwLock::new(Settings::default()),
        }
    }

    async fn ingest(&self, uri: Url, text: &str) {
        let settings = self.settings.read().await.clone();
        let doc = parse_package_json(text, &settings.check_sections);
        let queries: Vec<(String, Option<Version>)> = doc
            .deps
            .iter()
            .map(|d| (d.name.clone(), current_pinned(&d.range_str)))
            .collect();
        self.docs.insert(uri.clone(), doc);

        join_all(queries.iter().map(|(n, _)| self.registry.fetch(n))).await;
        if settings.audit {
            join_all(
                queries
                    .iter()
                    .filter_map(|(n, v)| v.as_ref().map(|c| self.auditor.audit(n, c))),
            )
            .await;
        }
        self.refresh(uri).await;
    }

    async fn refresh(&self, uri: Url) {
        let diags = self.diagnostics_for(&uri).await;
        self.client.publish_diagnostics(uri, diags, None).await;
    }

    /// Fetch the registry record + (optionally) the audit record for a dep.
    /// Returns `None` when the dep should be skipped wholesale.
    async fn fetch_dep(
        &self,
        dep: &DepEntry,
        want_audit: bool,
    ) -> Option<(CachedPackage, Option<AuditResult>)> {
        let pkg = self.registry.fetch(&dep.name).await?;
        if pkg.status != PackageStatus::Found {
            return None;
        }
        let audit = match (want_audit, current_pinned(&dep.range_str)) {
            (true, Some(current)) => self.auditor.audit(&dep.name, &current).await,
            _ => None,
        };
        Some((pkg, audit))
    }

    async fn diagnostics_for(&self, uri: &Url) -> Vec<Diagnostic> {
        let Some(doc) = self.docs.get(uri).map(|d| d.clone()) else {
            return Vec::new();
        };
        let settings = self.settings.read().await.clone();
        if !settings.show_updates {
            return Vec::new();
        }

        let mut out = Vec::new();
        for dep in &doc.deps {
            out.extend(self.dep_diagnostics(dep, &settings).await);
        }
        out
    }

    async fn dep_diagnostics(&self, dep: &DepEntry, settings: &Settings) -> Vec<Diagnostic> {
        if util::is_ignored(&dep.name, settings) {
            return Vec::new();
        }
        if dep.range.is_none() {
            return vec![diagnostics::invalid_semver(dep)];
        }
        let Some(pkg) = self.registry.fetch(&dep.name).await else {
            return Vec::new();
        };
        if pkg.status == PackageStatus::NotFound {
            return vec![diagnostics::not_found(dep)];
        }
        if !settings.audit {
            return Vec::new();
        }
        let Some(current) = current_pinned(&dep.range_str) else {
            return Vec::new();
        };
        let Some(audit) = self.auditor.audit(&dep.name, &current).await else {
            return Vec::new();
        };
        audit
            .advisories
            .iter()
            .map(|adv| diagnostics::advisory(dep, adv))
            .collect()
    }

    async fn inlay_hints_for(&self, uri: &Url) -> Vec<InlayHint> {
        let Some(doc) = self.docs.get(uri).map(|d| d.clone()) else {
            return Vec::new();
        };
        let settings = self.settings.read().await.clone();
        if !settings.show_updates {
            return Vec::new();
        }

        let mut hints = Vec::new();
        for dep in &doc.deps {
            if let Some(hint) = self.dep_inlay_hint(dep, &settings).await {
                hints.push(hint);
            }
        }
        hints
    }

    async fn dep_inlay_hint(&self, dep: &DepEntry, settings: &Settings) -> Option<InlayHint> {
        if util::is_ignored(&dep.name, settings) || dep.range.is_none() {
            return None;
        }
        let (pkg, audit) = self.fetch_dep(dep, settings.audit).await?;
        let current = current_pinned(&dep.range_str);

        let upgrade_label = current
            .as_ref()
            .and_then(|c| inlay::upgrade_label(dep, &pkg, c, settings));
        let cve_suffix = audit.as_ref().and_then(inlay::advisory_badge);

        let body = match (upgrade_label, cve_suffix) {
            (Some(u), Some(c)) => format!("{u}{c}"),
            (Some(u), None) => u,
            (None, Some(c)) => c,
            (None, None) => return None,
        };

        Some(InlayHint {
            position: util::hint_anchor(dep.value_range.end),
            label: InlayHintLabel::String(body),
            kind: Some(InlayHintKind::TYPE),
            text_edits: None,
            tooltip: Some(InlayHintTooltip::String(inlay::audit_tooltip(
                &dep.name,
                audit.as_ref(),
            ))),
            padding_left: Some(true),
            padding_right: None,
            data: None,
        })
    }

    async fn completions_for(&self, uri: &Url, pos: Position) -> Option<CompletionResponse> {
        let dep = self.dep_at(uri, pos)?;
        let pkg = self.registry.fetch(&dep.name).await?;
        if pkg.status != PackageStatus::Found {
            return None;
        }

        let (prefix, typed) = split_prefix(&dep.range_str);
        let latest = pkg.latest.as_ref().map(ToString::to_string);
        let items: Vec<CompletionItem> = pkg
            .versions
            .iter()
            .filter(|v| util::has_prefix(&v.to_string(), typed))
            .take(COMPLETION_LIMIT)
            .enumerate()
            .map(|(idx, v)| {
                let v_str = v.to_string();
                let is_latest = latest.as_deref() == Some(v_str.as_str());
                CompletionItem {
                    label: v_str.clone(),
                    label_details: Some(CompletionItemLabelDetails {
                        detail: None,
                        description: is_latest.then(|| "latest".into()),
                    }),
                    kind: Some(CompletionItemKind::VALUE),
                    sort_text: Some(format!("{idx:04}")),
                    filter_text: Some(v_str.clone()),
                    text_edit: Some(CompletionTextEdit::Edit(TextEdit {
                        range: dep.value_range,
                        new_text: format!("{prefix}{v_str}"),
                    })),
                    ..Default::default()
                }
            })
            .collect();
        Some(CompletionResponse::Array(items))
    }

    async fn hover_for(&self, uri: &Url, pos: Position) -> Option<Hover> {
        let dep = self.dep_at(uri, pos)?;
        let settings = self.settings.read().await.clone();
        let (pkg, audit) = self.fetch_dep(&dep, settings.audit).await?;
        Some(Hover {
            contents: HoverContents::Markup(MarkupContent {
                kind: MarkupKind::Markdown,
                value: hover::render(&dep, &pkg, audit.as_ref()),
            }),
            range: Some(dep.value_range),
        })
    }

    async fn code_actions_for(&self, uri: &Url, cursor: Position) -> Vec<CodeActionOrCommand> {
        let mut out = actions::bulk_update_actions(uri);
        let Some(doc) = self.docs.get(uri).map(|d| d.clone()) else {
            return out;
        };
        let settings = self.settings.read().await.clone();
        for dep in doc.deps.iter().filter(|d| position_in_range(cursor, d.value_range)) {
            out.extend(self.dep_actions(uri, dep, &settings).await);
        }
        out
    }

    async fn dep_actions(
        &self,
        uri: &Url,
        dep: &DepEntry,
        settings: &Settings,
    ) -> Vec<CodeActionOrCommand> {
        let Some((pkg, audit)) = self.fetch_dep(dep, settings.audit).await else {
            return Vec::new();
        };
        let Some(latest) = pkg.latest.as_ref() else {
            return Vec::new();
        };

        let mut out = actions::upgrade_actions(uri, dep, &pkg.versions, latest);
        if let Some(audit) = audit.as_ref() {
            if let Some(action) = actions::safe_upgrade_action(uri, dep, audit) {
                out.push(action);
            }
        }
        if let Some(home) = pkg.homepage.as_deref() {
            out.push(actions::open_url_action("Open homepage", home.into()));
        }
        if let Some(changelog) = pkg
            .repository_url
            .as_deref()
            .and_then(crate::registry::changelog_url)
        {
            out.push(actions::open_url_action("Open changelog", changelog));
        }
        out
    }

    async fn run_update_all(&self, uri: Url, tier: &str) {
        let Some(doc) = self.docs.get(&uri).map(|d| d.clone()) else {
            return;
        };
        let settings = self.settings.read().await.clone();
        let mut edits = Vec::new();
        for dep in &doc.deps {
            if let Some(edit) = self.tier_edit(dep, tier, &settings).await {
                edits.push(edit);
            }
        }
        if edits.is_empty() {
            return;
        }
        let mut changes = std::collections::HashMap::new();
        changes.insert(uri, edits);
        let _ = self
            .client
            .apply_edit(WorkspaceEdit {
                changes: Some(changes),
                document_changes: None,
                change_annotations: None,
            })
            .await;
    }

    async fn tier_edit(
        &self,
        dep: &DepEntry,
        tier: &str,
        settings: &Settings,
    ) -> Option<TextEdit> {
        if util::is_ignored(&dep.name, settings) || dep.range.is_none() {
            return None;
        }
        let pkg = self.registry.fetch(&dep.name).await?;
        if pkg.status != PackageStatus::Found {
            return None;
        }
        let current = current_pinned(&dep.range_str)?;
        let target = pick_tier_target(&pkg.versions, &current, tier)?;
        if target <= current || util::version_ignored(&dep.name, &target, settings) {
            return None;
        }
        let (prefix, _) = split_prefix(&dep.range_str);
        Some(TextEdit {
            range: dep.value_range,
            new_text: format!("{prefix}{target}"),
        })
    }

    async fn open_external(&self, url: &str) {
        let Ok(parsed) = Url::parse(url) else { return };
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

    fn dep_at(&self, uri: &Url, pos: Position) -> Option<DepEntry> {
        self.docs
            .get(uri)
            .map(|d| d.clone())?
            .deps
            .into_iter()
            .find(|d| position_in_range(pos, d.value_range))
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
                completion_provider: Some(CompletionOptions {
                    trigger_characters: Some(vec![
                        "\"".into(),
                        ".".into(),
                        "^".into(),
                        "~".into(),
                    ]),
                    resolve_provider: Some(false),
                    ..Default::default()
                }),
                hover_provider: Some(HoverProviderCapability::Simple(true)),
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
                    commands: vec![commands::OPEN_URL.into(), commands::UPDATE_ALL.into()],
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
        if cfg.is_null() {
            return;
        }
        if let Ok(parsed) = serde_json::from_value::<Settings>(cfg) {
            *self.settings.write().await = parsed;
        }
    }

    async fn shutdown(&self) -> LspResult<()> {
        Ok(())
    }

    async fn did_open(&self, params: DidOpenTextDocumentParams) {
        let uri = params.text_document.uri;
        if !util::is_package_json(&uri) {
            return;
        }
        self.ingest(uri, &params.text_document.text).await;
    }

    async fn did_change(&self, params: DidChangeTextDocumentParams) {
        let uri = params.text_document.uri;
        if !util::is_package_json(&uri) {
            return;
        }
        let Some(change) = params.content_changes.into_iter().last() else {
            return;
        };
        self.ingest(uri, &change.text).await;
    }

    async fn did_close(&self, params: DidCloseTextDocumentParams) {
        let uri = params.text_document.uri;
        self.docs.remove(&uri);
        self.client.publish_diagnostics(uri, vec![], None).await;
    }

    async fn did_change_configuration(&self, params: DidChangeConfigurationParams) {
        let Ok(parsed) = serde_json::from_value::<Settings>(params.settings) else {
            return;
        };
        *self.settings.write().await = parsed;
        let uris: Vec<Url> = self.docs.iter().map(|e| e.key().clone()).collect();
        for uri in uris {
            self.refresh(uri).await;
        }
    }

    async fn inlay_hint(&self, params: InlayHintParams) -> LspResult<Option<Vec<InlayHint>>> {
        let uri = params.text_document.uri;
        if !util::is_package_json(&uri) {
            return Ok(None);
        }
        Ok(Some(self.inlay_hints_for(&uri).await))
    }

    async fn completion(&self, params: CompletionParams) -> LspResult<Option<CompletionResponse>> {
        let uri = params.text_document_position.text_document.uri;
        if !util::is_package_json(&uri) {
            return Ok(None);
        }
        Ok(self
            .completions_for(&uri, params.text_document_position.position)
            .await)
    }

    async fn hover(&self, params: HoverParams) -> LspResult<Option<Hover>> {
        let uri = params.text_document_position_params.text_document.uri;
        if !util::is_package_json(&uri) {
            return Ok(None);
        }
        Ok(self
            .hover_for(&uri, params.text_document_position_params.position)
            .await)
    }

    async fn code_action(
        &self,
        params: CodeActionParams,
    ) -> LspResult<Option<CodeActionResponse>> {
        let uri = params.text_document.uri;
        if !util::is_package_json(&uri) {
            return Ok(None);
        }
        Ok(Some(
            self.code_actions_for(&uri, params.range.start).await,
        ))
    }

    async fn execute_command(&self, params: ExecuteCommandParams) -> LspResult<Option<Value>> {
        let mut args = params.arguments.into_iter();
        match params.command.as_str() {
            commands::OPEN_URL => {
                if let Some(Value::String(url)) = args.next() {
                    self.open_external(&url).await;
                }
                Ok(None)
            }
            commands::UPDATE_ALL => {
                let Some(Value::String(uri_s)) = args.next() else {
                    return Ok(None);
                };
                let tier = match args.next() {
                    Some(Value::String(s)) => s,
                    _ => "major".into(),
                };
                let Ok(uri) = Url::parse(&uri_s) else {
                    return Ok(None);
                };
                self.run_update_all(uri, &tier).await;
                Ok(None)
            }
            _ => Ok(None),
        }
    }
}
