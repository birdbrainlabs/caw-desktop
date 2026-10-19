//! Saving settings files safely.

use anyhow::{Context, Result};
use std::io::Write;
use std::path::Path;

/// Replaces `path` with `bytes` atomically (a temporary file in the same
/// folder, then a rename), readable only by this user (mode 600 on Unix).
pub fn write_private(path: &Path, bytes: &[u8]) -> Result<()> {
    let dir = path.parent().context("no folder")?;
    std::fs::create_dir_all(dir)?;
    let name = path.file_name().context("no file name")?.to_string_lossy();
    let tmp = dir.join(format!(".{name}.{}.tmp", std::process::id()));
    let _ = std::fs::remove_file(&tmp);
    let mut options = std::fs::OpenOptions::new();
    options.write(true).create_new(true);
    #[cfg(unix)]
    {
        use std::os::unix::fs::OpenOptionsExt;
        options.mode(0o600);
    }
    let result = (|| {
        let mut file = options.open(&tmp).with_context(|| format!("writing {}", tmp.display()))?;
        file.write_all(bytes)?;
        file.sync_all()?;
        std::fs::rename(&tmp, path).with_context(|| format!("saving {}", path.display()))
    })();
    if result.is_err() {
        let _ = std::fs::remove_file(&tmp);
    }
    result
}

#[cfg(test)]
mod tests {
    #[test]
    fn replaces_and_keeps_private() {
        let dir = std::env::temp_dir().join(format!("caw-fsutil-test-{}", std::process::id()));
        let path = dir.join("x.json");
        super::write_private(&path, b"one").unwrap();
        super::write_private(&path, b"two").unwrap();
        assert_eq!(std::fs::read(&path).unwrap(), b"two");
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            assert_eq!(std::fs::metadata(&path).unwrap().permissions().mode() & 0o777, 0o600);
        }
        assert_eq!(std::fs::read_dir(&dir).unwrap().count(), 1);
        std::fs::remove_dir_all(&dir).unwrap();
    }
}
