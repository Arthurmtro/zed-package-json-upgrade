use nodejs_semver::Version;
use regex::Regex;
use tower_lsp::lsp_types::{Position, Url};

use crate::settings::Settings;

pub fn is_package_json(uri: &Url) -> bool {
    uri.path()
        .rsplit('/')
        .next()
        .is_some_and(|f| f == "package.json")
}

pub fn is_ignored(name: &str, settings: &Settings) -> bool {
    settings
        .ignore_patterns
        .iter()
        .any(|pat| Regex::new(pat).is_ok_and(|re| re.is_match(name)))
}

pub fn version_ignored(name: &str, version: &Version, settings: &Settings) -> bool {
    settings
        .ignore_versions
        .get(name)
        .and_then(|rule| rule.parse::<nodejs_semver::Range>().ok())
        .is_some_and(|r| r.satisfies(version))
}

pub fn has_prefix(haystack: &str, needle: &str) -> bool {
    needle.is_empty() || haystack.starts_with(needle)
}

pub fn hint_anchor(end: Position) -> Position {
    Position {
        line: end.line,
        character: end.character.saturating_add(1),
    }
}
