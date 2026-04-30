# Package.json Upgrade — Zed extension

Inline npm dependency update hints for `package.json` in [Zed](https://zed.dev).
Inspired by the VS Code extension [`package-json-upgrade`](https://marketplace.visualstudio.com/items?itemName=codeandstuff.package-json-upgrade).

Features:

- **Inlay hint** after each outdated dependency, color-coded by upgrade size:
  - 🔴 major behind
  - 🟡 minor behind
  - 🟢 patch behind
- **Diagnostics**:
  - `Error` (red squiggle) — invalid semver in the version string.
  - `Warning` (yellow squiggle) — package not found in the npm registry.
  - `Hint` (no squiggle, sidebar marker only) — outdated dependency. The full upgrade tier is encoded in the inlay hint emoji, not in the diagnostic, so the file stays free of squiggle noise on perfectly valid but outdated deps.
- **Completions** inside the version string: typing `^`, `~`, `.`, or `"` lists the available versions for that package, with `latest` tagged.
- **Hover** on a version string shows the package description, latest version, license, homepage, and changelog link.
- **Quick-fix code actions** (Zed default `cmd-.` / `ctrl-.`):
  - **Do patch / minor / major upgrade to `<latest>`**
  - **Open homepage**
  - **Open changelog**
  - **Update all dependencies** (document-wide quick-fix)
- **Parallel registry prefetch** on open/change with a per-process concurrency cap (8 in-flight requests). Subsequent code actions, hovers, and completions are served from a 1-hour in-memory cache.

## Architecture

| Piece | What it does |
|-------|--------------|
| `extension.toml` + `src/lib.rs` (WASM) | Zed extension. Downloads the LSP binary from this repo's GitHub releases on first use, then registers it as a language server for `JSON` / `JSONC`. |
| `lsp/` (native binary) | A `tower-lsp` server that parses `package.json`, queries the npm registry (`https://registry.npmjs.org`), caches results, and emits diagnostics, inlay hints and code actions. |

The LSP attaches alongside Zed's built-in `json-language-server`. Both run, results merged.

## Install (once published)

`zed: extensions` → search **Package.json Upgrade** → install.

## Install from source (dev)

```sh
# 1. Build the LSP locally
cargo build --release --manifest-path lsp/Cargo.toml

# 2. Make sure Zed can find it: place it on PATH or symlink it as
#    ~/.local/bin/package-json-upgrade-lsp (the WASM downloader is bypassed
#    when a binary of the same name already exists on PATH — see `src/lib.rs`).

# 3. Install the extension as a dev extension:
#    Zed → cmd-shift-p → "zed: install dev extension" → pick this folder.
```

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
        "showUpdates": true
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
