use std::collections::HashMap;

use serde::Deserialize;

#[derive(Debug, Clone, Deserialize)]
#[serde(default, rename_all = "camelCase")]
pub struct Settings {
    pub ignore_patterns: Vec<String>,
    pub ignore_versions: HashMap<String, String>,
    pub check_sections: Vec<String>,
    pub show_updates: bool,
    pub audit: bool,
}

impl Default for Settings {
    fn default() -> Self {
        Self {
            ignore_patterns: Vec::new(),
            ignore_versions: HashMap::new(),
            check_sections: vec!["dependencies".into(), "devDependencies".into()],
            show_updates: true,
            audit: true,
        }
    }
}
