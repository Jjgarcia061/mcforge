//! The catalog is data, not code: connectors, use cases and models are TOML
//! files under `catalog/`. This module parses them and rejects anything
//! inconsistent, so a broken manifest fails CI instead of the user's setup.

use std::collections::{BTreeMap, HashMap, HashSet};
use std::path::Path;

use serde::{Deserialize, Serialize};

use crate::error::{Error, Result};
use crate::policy::Permission;

#[derive(Debug, Clone, Serialize, specta::Type)]
#[serde(rename_all = "camelCase")]
pub struct Catalog {
    pub connectors: Vec<Connector>,
    pub use_cases: Vec<UseCase>,
    pub models: Models,
    pub focuses: Vec<FocusPreset>,
}

#[derive(Debug, Clone, Deserialize, Serialize, specta::Type)]
#[serde(deny_unknown_fields, rename_all(serialize = "camelCase"))]
pub struct Connector {
    pub id: String,
    pub name: String,
    pub category: Category,
    pub description: String,
    pub homepage: Option<String>,
    /// Maintained by MCForge or reviewed in the lab against the current upstream release.
    #[serde(default)]
    pub verified: bool,
    pub server: Server,
    #[serde(default)]
    pub fields: Vec<Field>,
    pub tools: Vec<Tool>,
    /// Whether the wizard can check this configuration before saving. Derived at load time.
    #[serde(skip_deserializing, default)]
    pub testable: bool,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Deserialize, Serialize, specta::Type)]
#[serde(rename_all = "camelCase")]
pub enum Category {
    Files,
    Data,
    Reports,
    Web,
    Development,
    Apps,
}

/// How the MCP server is started. `{{field}}` placeholders are filled from user input.
#[derive(Debug, Clone, Deserialize, Serialize, specta::Type)]
#[serde(tag = "runtime", rename_all = "camelCase", deny_unknown_fields)]
pub enum Server {
    Npx(Package),
    Uvx(Package),
    Binary {
        command: String,
        #[serde(default)]
        args: Vec<String>,
        #[serde(default)]
        env: BTreeMap<String, String>,
    },
    Http {
        url: String,
        #[serde(default)]
        headers: BTreeMap<String, String>,
    },
    Builtin {
        module: BuiltinModule,
    },
}

/// Connectors implemented inside MCForge itself.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Deserialize, Serialize, specta::Type)]
#[serde(rename_all = "camelCase")]
pub enum BuiltinModule {
    Database,
    Reports,
}

#[derive(Debug, Clone, Deserialize, Serialize, specta::Type)]
#[serde(deny_unknown_fields)]
pub struct Package {
    pub package: String,
    #[serde(default)]
    pub args: Vec<String>,
    #[serde(default)]
    pub env: BTreeMap<String, String>,
}

#[derive(Debug, Clone, Deserialize, Serialize, specta::Type)]
#[serde(deny_unknown_fields, rename_all(serialize = "camelCase"))]
pub struct Field {
    pub key: String,
    pub label: String,
    pub kind: FieldKind,
    #[serde(default)]
    pub required: bool,
    pub placeholder: Option<String>,
    #[serde(default)]
    pub options: Vec<String>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Deserialize, Serialize, specta::Type)]
#[serde(rename_all = "camelCase")]
pub enum FieldKind {
    Text,
    Secret,
    Number,
    Url,
    Path,
    Paths,
    Select,
}

/// What the user typed or picked for one field.
#[derive(Debug, Clone, Deserialize, Serialize, specta::Type)]
#[serde(untagged)]
pub enum FieldValue {
    Text(String),
    List(Vec<String>),
}

impl FieldValue {
    pub fn as_text(&self) -> Option<&str> {
        match self {
            Self::Text(s) => Some(s),
            Self::List(_) => None,
        }
    }
}

/// Field values of one connector, keyed by field `key`.
pub type Values = HashMap<String, FieldValue>;

/// Outcome of a successful connection test, e.g. `PostgreSQL 17.6` and its tables.
#[derive(Debug, Clone, Serialize, specta::Type)]
pub struct TestReport {
    pub title: String,
    pub items: Vec<String>,
}

#[derive(Debug, Clone, Deserialize, Serialize, specta::Type)]
#[serde(deny_unknown_fields, rename_all(serialize = "camelCase"))]
pub struct Tool {
    pub name: String,
    pub risk: Risk,
    /// Derived from `risk` at load time; never written in manifests.
    #[serde(skip_deserializing, default)]
    pub default_permission: Permission,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Deserialize, Serialize, specta::Type)]
#[serde(rename_all = "camelCase")]
pub enum Risk {
    Read,
    Write,
    Destructive,
    Network,
    Send,
}

#[derive(Debug, Clone, Deserialize, Serialize, specta::Type)]
#[serde(deny_unknown_fields)]
pub struct UseCase {
    pub id: String,
    pub connectors: Vec<String>,
}

/// A built-in report focus; the UI names it, the model reads `instructions`.
#[derive(Debug, Clone, Deserialize, Serialize, specta::Type)]
#[serde(deny_unknown_fields)]
pub struct FocusPreset {
    pub id: String,
    pub instructions: String,
}

#[derive(Debug, Clone, Deserialize, Serialize, specta::Type)]
#[serde(deny_unknown_fields)]
pub struct Models {
    pub local: Vec<LocalModel>,
    pub api: Vec<ApiModel>,
}

#[derive(Debug, Clone, Deserialize, Serialize, specta::Type)]
#[serde(deny_unknown_fields, rename_all(serialize = "camelCase"))]
pub struct LocalModel {
    /// Ollama tag, e.g. `qwen3:8b`.
    pub id: String,
    pub name: String,
    pub params_b: f64,
    pub size_gb: f64,
    pub tools: ToolSkill,
}

#[derive(
    Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Deserialize, Serialize, specta::Type,
)]
#[serde(rename_all = "camelCase")]
pub enum ToolSkill {
    Poor,
    Fair,
    Good,
    Great,
}

#[derive(Debug, Clone, Deserialize, Serialize, specta::Type)]
#[serde(deny_unknown_fields)]
pub struct ApiModel {
    pub provider: crate::providers::Provider,
    pub id: String,
    pub name: String,
    #[serde(default)]
    pub recommended: bool,
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct UseCaseFile {
    use_cases: Vec<UseCase>,
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct FocusFile {
    focuses: Vec<FocusPreset>,
}

impl Catalog {
    pub fn load(dir: &Path) -> Result<Self> {
        let mut files: Vec<_> = std::fs::read_dir(dir.join("connectors"))?
            .filter_map(|e| e.ok().map(|e| e.path()))
            .filter(|p| p.extension().is_some_and(|x| x == "toml"))
            .collect();
        files.sort();

        let mut connectors = files
            .iter()
            .map(|p| parse::<Connector>(p))
            .collect::<Result<Vec<_>>>()?;
        for c in &mut connectors {
            c.testable = matches!(
                c.server,
                Server::Builtin {
                    module: BuiltinModule::Database
                }
            );
            for tool in &mut c.tools {
                tool.default_permission = Permission::default_for(tool.risk);
            }
        }

        let catalog = Self {
            connectors,
            use_cases: parse::<UseCaseFile>(&dir.join("use-cases.toml"))?.use_cases,
            models: parse(&dir.join("models.toml"))?,
            focuses: parse::<FocusFile>(&dir.join("report-focus.toml"))?.focuses,
        };
        catalog.validate()?;
        Ok(catalog)
    }

    pub fn connector(&self, id: &str) -> Result<&Connector> {
        self.connectors
            .iter()
            .find(|c| c.id == id)
            .ok_or_else(|| Error::Catalog(format!("unknown connector `{id}`")))
    }

    /// Cross-file consistency checks that serde cannot express. Reports every problem at once.
    fn validate(&self) -> Result<()> {
        let mut problems = Vec::new();
        let mut ids = HashSet::new();

        for c in &self.connectors {
            if !ids.insert(c.id.as_str()) {
                problems.push(format!("duplicate connector id `{}`", c.id));
            }
            if c.tools.is_empty() {
                problems.push(format!("`{}` declares no tools", c.id));
            }
            let mut tools = HashSet::new();
            for t in &c.tools {
                if !tools.insert(t.name.as_str()) {
                    problems.push(format!("`{}` declares tool `{}` twice", c.id, t.name));
                }
            }
            let keys: HashSet<_> = c.fields.iter().map(|f| f.key.as_str()).collect();
            for f in &c.fields {
                if (f.kind == FieldKind::Select) == f.options.is_empty() {
                    problems.push(format!(
                        "`{}.{}`: only select fields take options, and they need some",
                        c.id, f.key
                    ));
                }
            }
            for p in c.server.templates().flat_map(placeholders) {
                if !keys.contains(p) {
                    problems.push(format!(
                        "`{}` uses `{{{{{p}}}}}` but has no field `{p}`",
                        c.id
                    ));
                }
            }
        }

        for u in &self.use_cases {
            for c in u.connectors.iter().filter(|c| !ids.contains(c.as_str())) {
                problems.push(format!(
                    "use case `{}` references unknown connector `{c}`",
                    u.id
                ));
            }
        }

        let mut models = HashSet::new();
        for m in &self.models.local {
            if !models.insert(m.id.as_str()) {
                problems.push(format!("duplicate model `{}`", m.id));
            }
            if m.size_gb <= 0.0 || m.params_b <= 0.0 {
                problems.push(format!(
                    "model `{}` needs positive size and parameters",
                    m.id
                ));
            }
        }

        let mut focuses = HashSet::new();
        for f in &self.focuses {
            // Saved focuses get `custom-` ids, so presets must not use that prefix.
            if f.id.is_empty() || f.id.starts_with(crate::reports::focus::CUSTOM_PREFIX) {
                problems.push(format!("invalid focus id `{}`", f.id));
            }
            if !focuses.insert(f.id.as_str()) {
                problems.push(format!("duplicate focus `{}`", f.id));
            }
        }

        if problems.is_empty() {
            Ok(())
        } else {
            Err(Error::Catalog(problems.join("; ")))
        }
    }
}

impl Server {
    /// Every user-templated string of the launch spec.
    fn templates(&self) -> Box<dyn Iterator<Item = &str> + '_> {
        match self {
            Self::Npx(p) | Self::Uvx(p) => {
                Box::new(p.args.iter().chain(p.env.values()).map(String::as_str))
            }
            Self::Binary { command, args, env } => Box::new(
                std::iter::once(command)
                    .chain(args)
                    .chain(env.values())
                    .map(String::as_str),
            ),
            Self::Http { url, headers } => Box::new(
                std::iter::once(url)
                    .chain(headers.values())
                    .map(String::as_str),
            ),
            Self::Builtin { .. } => Box::new(std::iter::empty()),
        }
    }
}

/// Names inside `{{…}}` markers.
fn placeholders(s: &str) -> impl Iterator<Item = &str> {
    s.split("{{")
        .skip(1)
        .filter_map(|part| part.split_once("}}").map(|(name, _)| name.trim()))
}

fn parse<T: serde::de::DeserializeOwned>(path: &Path) -> Result<T> {
    let text = std::fs::read_to_string(path)
        .map_err(|e| Error::Catalog(format!("{}: {e}", path.display())))?;
    toml::from_str(&text).map_err(|e| Error::Catalog(format!("{}: {e}", path.display())))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn repo_catalog() -> Catalog {
        Catalog::load(&Path::new(env!("CARGO_MANIFEST_DIR")).join("../catalog")).unwrap()
    }

    #[test]
    fn shipped_catalog_is_valid() {
        let c = repo_catalog();
        assert!(c.connectors.len() >= 6);
        assert!(!c.models.local.is_empty());
    }

    #[test]
    fn default_permissions_follow_risk() {
        let c = repo_catalog();
        let fs = c.connectors.iter().find(|c| c.id == "filesystem").unwrap();
        let perm = |name: &str| {
            fs.tools
                .iter()
                .find(|t| t.name == name)
                .unwrap()
                .default_permission
        };
        assert_eq!(perm("read_text_file"), Permission::Allow);
        assert_eq!(perm("write_file"), Permission::Ask);
        assert_eq!(perm("move_file"), Permission::Deny);
    }

    #[test]
    fn only_the_database_connector_is_testable_for_now() {
        let c = repo_catalog();
        let testable: Vec<_> = c
            .connectors
            .iter()
            .filter(|c| c.testable)
            .map(|c| c.id.as_str())
            .collect();
        assert_eq!(testable, ["database"]);
    }

    #[test]
    fn extracts_placeholders() {
        let found: Vec<_> = placeholders("Bearer {{token}} and {{ other }}").collect();
        assert_eq!(found, ["token", "other"]);
    }

    #[test]
    fn rejects_unknown_placeholder() {
        let mut c = repo_catalog();
        let fs = c
            .connectors
            .iter_mut()
            .find(|c| c.id == "filesystem")
            .unwrap();
        fs.fields.clear();
        let err = c.validate().unwrap_err().to_string();
        assert!(err.contains("{{folders}}"), "{err}");
    }

    #[test]
    fn rejects_unknown_manifest_keys() {
        let toml = "id='x'\nname='x'\ncategory='files'\ndescription='x'\ntypo=1\n[server]\nruntime='builtin'\nmodule='x'\n[[tools]]\nname='t'\nrisk='read'\n";
        assert!(toml::from_str::<Connector>(toml).is_err());
    }
}
