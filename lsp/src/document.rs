use std::sync::OnceLock;

use nodejs_semver::{Range, Version};
use regex::Regex;
use tower_lsp::lsp_types::{Position, Range as LspRange};

#[derive(Debug, Clone)]
pub struct DepEntry {
    pub name: String,
    pub range_str: String,
    pub range: Option<Range>,
    pub value_range: LspRange,
}

#[derive(Debug, Clone, Default)]
pub struct DocState {
    pub deps: Vec<DepEntry>,
}

pub fn parse_package_json(text: &str, sections: &[String]) -> DocState {
    let mut deps = Vec::new();
    for cap in section_regex().captures_iter(text) {
        let key = cap.name("key").unwrap().as_str();
        if !sections.iter().any(|s| s == key) {
            continue;
        }
        let open_brace = cap.get(0).unwrap().end() - 1;
        let Some(close_brace) = find_matching_brace(text, open_brace) else {
            continue;
        };
        let inner = &text[open_brace + 1..close_brace];
        let inner_offset = open_brace + 1;
        for ecap in entry_regex().captures_iter(inner) {
            let name = ecap.name("name").unwrap().as_str().to_string();
            let ver_match = ecap.name("ver").unwrap();
            let range_str = ver_match.as_str().to_string();
            let abs_start = inner_offset + ver_match.start();
            let abs_end = inner_offset + ver_match.end();
            let value_range = LspRange {
                start: offset_to_position(text, abs_start),
                end: offset_to_position(text, abs_end),
            };
            if range_str == "workspace:*" || range_str.starts_with("workspace:") {
                continue;
            }
            if range_str.starts_with("file:")
                || range_str.starts_with("link:")
                || range_str.starts_with("git+")
                || range_str.starts_with("github:")
                || range_str.starts_with("npm:")
                || range_str.contains("://")
            {
                continue;
            }
            let parsed_range = range_str.parse::<Range>().ok();
            deps.push(DepEntry {
                name,
                range_str,
                range: parsed_range,
                value_range,
            });
        }
    }
    DocState { deps }
}

pub fn split_prefix(range_str: &str) -> (&str, &str) {
    let trimmed = range_str.trim_start();
    let bytes = trimmed.as_bytes();
    let prefix_len = match bytes.first() {
        Some(b'^') | Some(b'~') | Some(b'>') | Some(b'<') | Some(b'=') => {
            if bytes.len() > 1 && (bytes[1] == b'=' || bytes[1] == b'<' || bytes[1] == b'>') {
                2
            } else {
                1
            }
        }
        _ => 0,
    };
    (&trimmed[..prefix_len], &trimmed[prefix_len..])
}

pub fn current_pinned(range_str: &str) -> Option<Version> {
    let (_, rest) = split_prefix(range_str);
    rest.parse().ok()
}

pub fn upgrade_kind(current: Option<&Version>, target: &Version) -> &'static str {
    match current {
        Some(c) if c.major != target.major => "major",
        Some(c) if c.minor != target.minor => "minor",
        Some(_) => "patch",
        None => "upgrade",
    }
}

/// Highest stable version that fits inside `tier` relative to `current`:
/// * `patch` — same `major.minor`, higher patch
/// * `minor` — same major, higher minor or patch
/// * `major` — absolute latest stable
pub fn pick_tier_target(versions: &[Version], current: &Version, tier: &str) -> Option<Version> {
    let mut stable = versions.iter().filter(|v| !v.is_prerelease());
    match tier {
        "patch" => stable
            .find(|v| v.major == current.major && v.minor == current.minor && *v > current)
            .cloned(),
        "minor" => stable
            .find(|v| v.major == current.major && *v > current)
            .cloned(),
        "major" => stable.next().cloned(),
        _ => None,
    }
}

pub fn position_in_range(p: Position, r: LspRange) -> bool {
    (p.line > r.start.line || (p.line == r.start.line && p.character >= r.start.character))
        && (p.line < r.end.line || (p.line == r.end.line && p.character <= r.end.character))
}

fn find_matching_brace(text: &str, open: usize) -> Option<usize> {
    let bytes = text.as_bytes();
    if bytes.get(open) != Some(&b'{') {
        return None;
    }
    let mut depth = 0i32;
    let mut in_string = false;
    let mut escape = false;
    for (i, &b) in bytes.iter().enumerate().skip(open) {
        if escape {
            escape = false;
            continue;
        }
        match b {
            b'\\' if in_string => escape = true,
            b'"' => in_string = !in_string,
            b'{' if !in_string => depth += 1,
            b'}' if !in_string => {
                depth -= 1;
                if depth == 0 {
                    return Some(i);
                }
            }
            _ => {}
        }
    }
    None
}

fn offset_to_position(text: &str, offset: usize) -> Position {
    let mut line = 0u32;
    let mut col = 0u32;
    for (i, ch) in text.char_indices() {
        if i >= offset {
            return Position {
                line,
                character: col,
            };
        }
        if ch == '\n' {
            line += 1;
            col = 0;
        } else {
            col += ch.len_utf16() as u32;
        }
    }
    Position {
        line,
        character: col,
    }
}

fn section_regex() -> &'static Regex {
    static R: OnceLock<Regex> = OnceLock::new();
    R.get_or_init(|| Regex::new(r#""(?P<key>[A-Za-z]+[A-Za-z0-9_-]*)"\s*:\s*\{"#).unwrap())
}

fn entry_regex() -> &'static Regex {
    static R: OnceLock<Regex> = OnceLock::new();
    R.get_or_init(|| {
        Regex::new(r#""(?P<name>(?:@[^"\s/]+/)?[^"\s]+)"\s*:\s*"(?P<ver>[^"]*)""#).unwrap()
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_dependencies_and_dev() {
        let text = r#"{
  "name": "demo",
  "dependencies": { "react": "^18.0.0", "@scope/pkg": "1.2.3" },
  "devDependencies": { "vitest": "~1.0.0" },
  "peerDependencies": { "ignored": "1.0.0" }
}"#;
        let sections = vec!["dependencies".into(), "devDependencies".into()];
        let doc = parse_package_json(text, &sections);
        let names: Vec<_> = doc.deps.iter().map(|d| d.name.as_str()).collect();
        assert_eq!(names, vec!["react", "@scope/pkg", "vitest"]);
    }

    #[test]
    fn split_prefix_handles_caret_tilde_and_compound() {
        assert_eq!(split_prefix("^1.2.3"), ("^", "1.2.3"));
        assert_eq!(split_prefix("~1.2.3"), ("~", "1.2.3"));
        assert_eq!(split_prefix(">=1.0.0"), (">=", "1.0.0"));
        assert_eq!(split_prefix("1.2.3"), ("", "1.2.3"));
    }

    #[test]
    fn upgrade_kind_classifies_bump() {
        let v = |s: &str| s.parse::<Version>().unwrap();
        assert_eq!(upgrade_kind(Some(&v("1.0.0")), &v("2.0.0")), "major");
        assert_eq!(upgrade_kind(Some(&v("1.0.0")), &v("1.1.0")), "minor");
        assert_eq!(upgrade_kind(Some(&v("1.0.0")), &v("1.0.1")), "patch");
        assert_eq!(upgrade_kind(None, &v("1.0.0")), "upgrade");
    }
}
