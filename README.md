# Package.json Upgrade — Zed extension

Inline npm dependency update hints for `package.json` in [Zed](https://zed.dev).
Inspired by the VS Code extension [`package-json-upgrade`](https://marketplace.visualstudio.com/items?itemName=codeandstuff.package-json-upgrade).

Features:

- **Inlay hint** after each dependency, listing every distinct upgrade tier available — e.g. ` 🟢 17.0.2 · 🟡 17.9.5 · 🔴 19.5.7` for `^17.0.0` when patch / minor / major targets all differ. Tiers that resolve to the same version are de-duplicated, so a dep with only a major bump available shows just `🔴 X.Y.Z`. A `⚠ Nc:Nh:Nm:Nl` suffix is appended when the pinned version has known security advisories (counts of critical / high / moderate / low).
- **Diagnostics** for genuine problems only:
  - `Error` (red squiggle) — invalid semver in the version string, or critical/high security advisory affecting the pinned version.
  - `Warning` (yellow squiggle) — package not found in the npm registry, or moderate security advisory.
  - `Information` — low security advisory.
  - Outdated dependencies do not emit a diagnostic; the colored inlay hint already conveys the upgrade tier, and reserving squiggles for real errors keeps the file readable when many deps happen to be behind latest.
- **Completions** inside the version string: typing `^`, `~`, `.`, or `"` lists the available versions for that package, with `latest` tagged.
- **Hover** on a version string shows the package description, latest version, license, homepage, changelog link, and a per-advisory list when CVEs apply to the pinned version.
- **Quick-fix code actions** (Zed default `cmd-.` / `ctrl-.`):
  - **Do patch / minor / major upgrade to `<latest>`**
  - **Open homepage**
  - **Open changelog**
  - **Update to first non-vulnerable version** (when audit advisories apply to the pinned version)
  - **Update all dependencies (patch / minor / major)** (document-wide quick-fix; pick the safety tier you want)
- **Parallel registry prefetch** on open/change with a per-process concurrency cap (8 in-flight requests). Subsequent code actions, hovers, and completions are served from a 1-hour in-memory cache.

<img width="496" height="151" alt="image" src="https://github.com/user-attachments/assets/1714e4b8-c122-4a0a-9159-02b1bd9a8a6e" />

<img width="684" height="445" alt="image" src="https://github.com/user-attachments/assets/758d4fb5-2329-485e-9356-b5e006275dce" />

<img width="260" height="165" alt="image" src="https://github.com/user-attachments/assets/9a870492-d4e9-4e85-9fc7-65e0dc701381" />

## Architecture

| Piece | What it does |
|-------|--------------|
| `extension.toml` + `src/lib.rs` (WASM) | Zed extension. Downloads the LSP binary from this repo's GitHub releases on first use, then registers it as a language server for `JSON` / `JSONC`. |
| `lsp/` (native binary) | A `tower-lsp` server that parses `package.json`, queries the npm registry (`https://registry.npmjs.org`), caches results, and emits diagnostics, inlay hints and code actions. |

The LSP attaches alongside Zed's built-in `json-language-server`. Both run, results merged.

## Install (once published)

`zed: extensions` → search **Package.json Upgrade** → install.

## Install from source (dev)

In Zed: `cmd-shift-p` → `zed: install dev extension` → pick this folder. Zed compiles the WASM and the extension downloads the matching LSP binary from the latest GitHub release on first use.

To iterate on the LSP without round-tripping through a release, build and run it locally:

```sh
cargo build --release --manifest-path lsp/Cargo.toml
# point Zed at the local binary via ~/.config/zed/settings.json:
#   "lsp": { "package-json-upgrade": { "binary": { "path": "/abs/path/to/target/release/package-json-upgrade-lsp" } } }
```

## Enabling inlay hints (without enabling them everywhere)

The colored markers (`🔴 25.6.0`, `⚠ 1h`, …) are LSP inlay hints. Zed defaults inlay hints to off because globally enabling them lights up every TypeScript / Rust / Python type hint too — which most people don't want. Enable them only for JSON in your `settings.json`:

```jsonc
{
  // leave this off to keep TS/Rust/Python type hints out of the way
  "inlay_hints": { "enabled": false },

  "languages": {
    "JSON": {
      "inlay_hints": { "enabled": true }
    },
    "JSONC": {
      "inlay_hints": { "enabled": true }
    }
  }
}
```

This scopes inlay hints to JSON / JSONC buffers. Zed's built-in `json-language-server` does not emit inlay hints, so in practice the only extension this affects is `package-json-upgrade`.

The diagnostics, hovers, and code actions work without any inlay-hint config — they're standard LSP features.

## Settings

In Zed `settings.json`:

```jsonc
{
  "lsp": {
    "package-json-upgrade": {
      "settings": {
        "ignorePatterns": ["^@types/.+$"],
        "ignoreVersions": {
          "@types/node": ">18"
        },
        "checkSections": ["dependencies", "devDependencies", "peerDependencies"],
        "showUpdates": true,
        "audit": true
      }
    }
  }
}
```

| Key | Type | Default | Effect |
|-----|------|---------|--------|
| `ignorePatterns` | `string[]` (regex) | `[]` | Hide updates for matching package names. |
| `ignoreVersions` | `{ [name]: semverRange }` | `{}` | Hide latest versions matching the range. Useful to pin a major. |
| `checkSections` | `string[]` | `["dependencies", "devDependencies"]` | Which top-level sections to scan. |
| `showUpdates` | `boolean` | `true` | Disable to silence the extension globally. |
| `audit` | `boolean` | `true` | Query [OSV.dev](https://osv.dev) for security advisories on the pinned version of each dependency. Set to `false` to disable audit network calls and CVE markers entirely. |

## Publishing to the Zed extension registry

1. Tag the repo (`git tag v0.0.1 && git push --tags`). The release workflow builds and uploads LSP binaries for macOS (arm64+x64), Linux (arm64+x64) and Windows (x64).
2. Bump `version` in `extension.toml` to match the tag.
3. Fork [`zed-industries/extensions`](https://github.com/zed-industries/extensions).
4. `git submodule add https://github.com/Arthurmtro/zed-package-json-upgrade.git extensions/package-json-upgrade`
5. Add to `extensions.toml`:
   ```toml
   [package-json-upgrade]
   submodule = "extensions/package-json-upgrade"
   version = "0.0.1"
   ```
6. `pnpm sort-extensions`, commit, open PR.

For subsequent updates: `git submodule update --remote extensions/package-json-upgrade` then bump `version` in `extensions.toml`.

## License

Apache-2.0
