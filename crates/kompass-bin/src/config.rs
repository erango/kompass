//! Persisted UI preferences (last cluster context, namespace, theme, kind).
//! Stored as JSON in the platform config dir. Best-effort: failures are ignored.

use serde::{Deserialize, Serialize};
use std::path::PathBuf;

#[derive(Serialize, Deserialize, Clone)]
pub struct Prefs {
    /// Kubeconfig context these prefs were last saved against.
    pub context: String,
    /// Last namespace filter (None = all namespaces).
    pub namespace: Option<String>,
    /// Last resource kind id ("pods" / "deployments").
    pub kind: String,
    /// Theme mode: "system" (follow OS), "dark", or "light".
    #[serde(default = "default_theme")]
    pub theme: String,
    /// Last sort column id ("name" / "namespace" / "status" / "age" / "col:N").
    #[serde(default = "default_sort_key")]
    pub sort_key: String,
    /// Sort ascending.
    #[serde(default = "default_true")]
    pub sort_asc: bool,
    /// Open namespace views (None = all namespaces). Empty → derived from `namespace`.
    #[serde(default)]
    pub ns_views: Vec<Option<String>>,
    /// Active namespace view index.
    #[serde(default)]
    pub ns_active: usize,
    /// Hidden column keys per kind id (default-hidden: "namespace").
    #[serde(default)]
    pub columns: std::collections::HashMap<String, Vec<String>>,
    /// Page to open on launch: "overview" or a kind id (e.g. "deployments.apps").
    #[serde(default = "default_page")]
    pub default_page: String,
    /// Cluster contexts pinned to the top of the switcher.
    #[serde(default)]
    pub pinned_clusters: Vec<String>,
    /// Namespace views per cluster context: (views, active index).
    #[serde(default)]
    pub ns_views_by_ctx: std::collections::HashMap<String, (Vec<Option<String>>, usize)>,
}

fn default_page() -> String {
    "deployments.apps".into()
}

fn default_theme() -> String {
    "dark".into()
}

fn default_sort_key() -> String {
    "name".into()
}
fn default_true() -> bool {
    true
}

impl Default for Prefs {
    fn default() -> Self {
        Prefs {
            context: String::new(),
            namespace: None,
            kind: "pods".into(),
            theme: default_theme(),
            sort_key: default_sort_key(),
            sort_asc: true,
            ns_views: Vec::new(),
            ns_active: 0,
            columns: std::collections::HashMap::new(),
            default_page: default_page(),
            pinned_clusters: Vec::new(),
            ns_views_by_ctx: std::collections::HashMap::new(),
        }
    }
}

fn prefs_path() -> Option<PathBuf> {
    directories::ProjectDirs::from("dev", "Kompass", "Kompass")
        .map(|d| d.config_dir().join("prefs.json"))
}

pub fn load() -> Prefs {
    prefs_path()
        .and_then(|p| std::fs::read_to_string(p).ok())
        .and_then(|s| serde_json::from_str(&s).ok())
        .unwrap_or_default()
}

pub fn save(prefs: &Prefs) {
    let Some(path) = prefs_path() else { return };
    if let Some(parent) = path.parent() {
        let _ = std::fs::create_dir_all(parent);
    }
    if let Ok(json) = serde_json::to_string_pretty(prefs) {
        let _ = std::fs::write(path, json);
    }
}
