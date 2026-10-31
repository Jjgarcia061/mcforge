//! The user's saved setup: engine, connections and permission rules. Stored as
//! JSON in the app's config folder; values of `secret` fields go to the OS
//! keychain instead and are filled back in on load.

use std::collections::HashMap;
use std::path::Path;

use serde::{Deserialize, Serialize};

use crate::atomic;
use crate::catalog::{Catalog, FieldKind, FieldValue, Values};
use crate::error::{Error, Result};
use crate::policy::Permission;
use crate::providers::Provider;
use crate::secrets;

#[derive(Debug, Clone, PartialEq, Eq, Deserialize, Serialize, specta::Type)]
#[serde(tag = "kind", rename_all = "camelCase")]
pub enum Engine {
    Local { model: String },
    Api { provider: Provider, model: String },
}

#[derive(Debug, Clone, Deserialize, Serialize, specta::Type)]
#[serde(rename_all = "camelCase")]
pub struct Setup {
    pub engine: Engine,
    pub use_case: Option<String>,
    pub connectors: Vec<String>,
    #[serde(default)]
    pub values: HashMap<String, Values>,
    #[serde(default)]
    pub permissions: HashMap<String, HashMap<String, Permission>>,
}

impl Setup {
    /// A field's text value, for connectors the user selected.
    pub fn text(&self, connector: &str, key: &str) -> Option<&str> {
        self.connectors
            .iter()
            .any(|c| c == connector)
            .then_some(())?;
        self.values
            .get(connector)?
            .get(key)?
            .as_text()
            .filter(|v| !v.trim().is_empty())
    }

    /// Drops values of unselected connectors and moves secret values out, keyed by keychain account.
    fn split_secrets(&mut self, catalog: &Catalog) -> Result<Vec<(String, String)>> {
        self.values.retain(|id, _| self.connectors.contains(id));
        let mut secrets = Vec::new();
        for id in &self.connectors {
            let connector = catalog.connector(id)?;
            let Some(values) = self.values.get_mut(id) else {
                continue;
            };
            for field in connector
                .fields
                .iter()
                .filter(|f| f.kind == FieldKind::Secret)
            {
                if let Some(FieldValue::Text(secret)) = values.remove(&field.key) {
                    secrets.push((account(id, &field.key), secret));
                }
            }
        }
        Ok(secrets)
    }
}

fn account(connector: &str, field: &str) -> String {
    format!("connector:{connector}:{field}")
}

pub fn load(path: &Path, catalog: &Catalog) -> Result<Option<Setup>> {
    let json = match std::fs::read(path) {
        Ok(json) => json,
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => return Ok(None),
        Err(e) => return Err(e.into()),
    };
    let mut setup: Setup =
        serde_json::from_slice(&json).map_err(|e| Error::Io(format!("{}: {e}", path.display())))?;
    // Connectors removed from the catalog since the last save are dropped, not fatal.
    setup.connectors.retain(|id| catalog.connector(id).is_ok());
    for id in &setup.connectors {
        for field in catalog.connector(id)?.fields.iter() {
            if field.kind == FieldKind::Secret
                && let Some(secret) = secrets::get(&account(id, &field.key))?
            {
                setup
                    .values
                    .entry(id.clone())
                    .or_default()
                    .insert(field.key.clone(), FieldValue::Text(secret));
            }
        }
    }
    Ok(Some(setup))
}

pub fn save(path: &Path, setup: &Setup, catalog: &Catalog) -> Result<()> {
    let mut public = setup.clone();
    for (account, secret) in public.split_secrets(catalog)? {
        secrets::set(&account, &secret)?;
    }
    let json = serde_json::to_vec_pretty(&public).map_err(|e| Error::Io(e.to_string()))?;
    atomic::write(path, &json)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn catalog() -> Catalog {
        Catalog::load(&Path::new(env!("CARGO_MANIFEST_DIR")).join("../catalog")).unwrap()
    }

    fn text(v: &str) -> FieldValue {
        FieldValue::Text(v.into())
    }

    #[test]
    fn secrets_never_reach_the_file() {
        let mut setup = Setup {
            engine: Engine::Local {
                model: "qwen3:8b".into(),
            },
            use_case: Some("data".into()),
            connectors: vec!["database".into()],
            values: HashMap::from([
                (
                    "database".into(),
                    Values::from([
                        ("driver".into(), text("postgres")),
                        ("url".into(), text("postgres://u:secret@h/db")),
                    ]),
                ),
                (
                    "filesystem".into(),
                    Values::from([("folders".into(), FieldValue::List(vec!["/x".into()]))]),
                ),
            ]),
            permissions: HashMap::new(),
        };
        let secrets = setup.split_secrets(&catalog()).unwrap();
        assert_eq!(
            secrets,
            [(
                "connector:database:url".into(),
                "postgres://u:secret@h/db".into()
            )]
        );
        let json = serde_json::to_string(&setup).unwrap();
        assert!(!json.contains("secret@"), "{json}");
        assert!(
            !json.contains("filesystem"),
            "unselected connectors are dropped: {json}"
        );
        assert_eq!(setup.text("database", "driver"), Some("postgres"));
    }

    #[test]
    fn engine_matches_the_ui_shape() {
        let engine = Engine::Api {
            provider: Provider::Anthropic,
            model: "claude-sonnet-5-5".into(),
        };
        assert_eq!(
            serde_json::to_value(&engine).unwrap(),
            serde_json::json!({ "kind": "api", "provider": "anthropic", "model": "claude-sonnet-5-5" })
        );
    }
}
