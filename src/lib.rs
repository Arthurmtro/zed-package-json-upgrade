use zed_extension_api::{
    self as zed, DownloadedFileType, GithubReleaseOptions, LanguageServerId, Result,
};

const REPO: &str = "Arthurmtro/zed-package-json-upgrade";
const BIN_NAME: &str = "package-json-upgrade-lsp";

struct PackageJsonUpgradeExtension {
    cached_binary_path: Option<String>,
}

impl PackageJsonUpgradeExtension {
    fn binary_path(&mut self, id: &LanguageServerId) -> Result<String> {
        if let Some(path) = &self.cached_binary_path {
            if std::path::Path::new(path).exists() {
                return Ok(path.clone());
            }
        }

        zed::set_language_server_installation_status(
            id,
            &zed::LanguageServerInstallationStatus::CheckingForUpdate,
        );

        let release = zed::latest_github_release(
            REPO,
            GithubReleaseOptions {
                require_assets: true,
                pre_release: false,
            },
        )?;

        let (platform, arch) = zed::current_platform();
        let asset_stem = format!(
            "{BIN_NAME}-{}-{}",
            match arch {
                zed::Architecture::Aarch64 => "aarch64",
                zed::Architecture::X8664 => "x86_64",
                zed::Architecture::X86 => "x86",
            },
            match platform {
                zed::Os::Mac => "apple-darwin",
                zed::Os::Linux => "unknown-linux-gnu",
                zed::Os::Windows => "pc-windows-msvc",
            },
        );
        let archive_ext = match platform {
            zed::Os::Windows => "zip",
            _ => "tar.gz",
        };
        let asset_name = format!("{asset_stem}.{archive_ext}");

        let asset = release
            .assets
            .iter()
            .find(|a| a.name == asset_name)
            .ok_or_else(|| format!("no asset named {asset_name} in release {}", release.version))?;

        let version_dir = format!("{BIN_NAME}-{}", release.version);
        let bin_filename = match platform {
            zed::Os::Windows => format!("{BIN_NAME}.exe"),
            _ => BIN_NAME.to_string(),
        };
        let bin_path = format!("{version_dir}/{bin_filename}");

        if !std::path::Path::new(&bin_path).exists() {
            zed::set_language_server_installation_status(
                id,
                &zed::LanguageServerInstallationStatus::Downloading,
            );

            let file_type = match platform {
                zed::Os::Windows => DownloadedFileType::Zip,
                _ => DownloadedFileType::GzipTar,
            };
            zed::download_file(&asset.download_url, &version_dir, file_type)
                .map_err(|e| format!("failed to download {asset_name}: {e}"))?;

            zed::make_file_executable(&bin_path)?;
            prune_old_versions(&version_dir);
        }

        self.cached_binary_path = Some(bin_path.clone());
        Ok(bin_path)
    }
}

impl zed::Extension for PackageJsonUpgradeExtension {
    fn new() -> Self {
        Self {
            cached_binary_path: None,
        }
    }

    fn language_server_command(
        &mut self,
        id: &LanguageServerId,
        _worktree: &zed::Worktree,
    ) -> Result<zed::Command> {
        Ok(zed::Command {
            command: self.binary_path(id)?,
            args: vec![],
            env: Default::default(),
        })
    }

    fn language_server_workspace_configuration(
        &mut self,
        _id: &LanguageServerId,
        worktree: &zed::Worktree,
    ) -> Result<Option<zed::serde_json::Value>> {
        let settings = zed::settings::LspSettings::for_worktree("package-json-upgrade", worktree)
            .ok()
            .and_then(|s| s.settings)
            .unwrap_or(zed::serde_json::json!({}));
        Ok(Some(zed::serde_json::json!({
            "package-json-upgrade": settings,
        })))
    }
}

fn prune_old_versions(keep: &str) {
    let Ok(entries) = std::fs::read_dir(".") else {
        return;
    };
    for entry in entries.flatten() {
        let Some(name) = entry.file_name().to_str().map(str::to_owned) else {
            continue;
        };
        if name.starts_with(BIN_NAME) && name != keep {
            let _ = std::fs::remove_dir_all(entry.path());
        }
    }
}

zed::register_extension!(PackageJsonUpgradeExtension);
