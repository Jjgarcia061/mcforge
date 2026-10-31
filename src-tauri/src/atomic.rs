//! File writes that never leave a half-written file behind, even if the app
//! stops halfway: the bytes go to a sibling file that then replaces the target.

use std::path::Path;

use crate::error::Result;

pub fn write(path: &Path, bytes: &[u8]) -> Result<()> {
    if let Some(dir) = path.parent() {
        std::fs::create_dir_all(dir)?;
    }
    let partial = path.with_extension("partial");
    std::fs::write(&partial, bytes)?;
    std::fs::rename(partial, path)?;
    Ok(())
}
