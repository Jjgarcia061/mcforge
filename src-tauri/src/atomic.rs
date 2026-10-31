//! File writes that never leave a half-written file behind, even if the app
//! stops halfway: the bytes go to a sibling file that then replaces the target.

use std::path::{Path, PathBuf};
use std::sync::{Mutex, MutexGuard, PoisonError};

use serde::Serialize;
use serde::de::DeserializeOwned;

use crate::error::{Error, Result};

pub fn write(path: &Path, bytes: &[u8]) -> Result<()> {
    if let Some(dir) = path.parent() {
        std::fs::create_dir_all(dir)?;
    }
    let partial = path.with_extension("partial");
    std::fs::write(&partial, bytes)?;
    std::fs::rename(partial, path)?;
    Ok(())
}

/// A small JSON document kept in memory and saved after every change.
pub struct Stored<T> {
    path: PathBuf,
    value: Mutex<T>,
}

impl<T: Serialize + DeserializeOwned + Default> Stored<T> {
    /// A missing or unreadable file starts from the default; it never stops the app.
    pub fn load(path: PathBuf) -> Self {
        let value = std::fs::read(&path)
            .ok()
            .and_then(|json| serde_json::from_slice(&json).ok())
            .unwrap_or_default();
        Self {
            path,
            value: Mutex::new(value),
        }
    }

    pub fn read<R>(&self, f: impl FnOnce(&T) -> R) -> R {
        f(&self.lock())
    }

    /// Applies `f`, then saves. The change is kept in memory even if saving fails.
    pub fn update<R>(&self, f: impl FnOnce(&mut T) -> R) -> Result<R> {
        let mut value = self.lock();
        let out = f(&mut value);
        let json = serde_json::to_vec_pretty(&*value).map_err(|e| Error::Io(e.to_string()))?;
        write(&self.path, &json)?;
        Ok(out)
    }

    fn lock(&self) -> MutexGuard<'_, T> {
        // Every update leaves a valid value, so one interrupted by a panic is still usable.
        self.value.lock().unwrap_or_else(PoisonError::into_inner)
    }
}
