//! API keys and connector secrets live in the OS keychain, never on disk or in logs.

use crate::error::Result;

const SERVICE: &str = "io.github.mcforge";

fn entry(account: &str) -> Result<keyring::Entry> {
    Ok(keyring::Entry::new(SERVICE, account)?)
}

pub fn set(account: &str, secret: &str) -> Result<()> {
    Ok(entry(account)?.set_password(secret)?)
}

pub fn get(account: &str) -> Result<Option<String>> {
    match entry(account)?.get_password() {
        Ok(secret) => Ok(Some(secret)),
        Err(keyring::Error::NoEntry) => Ok(None),
        Err(e) => Err(e.into()),
    }
}
