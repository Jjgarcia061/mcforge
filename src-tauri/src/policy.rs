//! Permission gate. The host enforces these decisions on every tool call; the
//! model never gets to decide. Enforcement and auditing land with the MCP host.

use serde::{Deserialize, Serialize};

use crate::catalog::Risk;

#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Deserialize, Serialize, specta::Type)]
#[serde(rename_all = "camelCase")]
pub enum Permission {
    Allow,
    #[default]
    Ask,
    Deny,
}

impl Permission {
    /// Safe defaults: reading is free, anything with side effects asks, destruction is off.
    pub fn default_for(risk: Risk) -> Self {
        match risk {
            Risk::Read => Self::Allow,
            Risk::Write | Risk::Network | Risk::Send => Self::Ask,
            Risk::Destructive => Self::Deny,
        }
    }
}
