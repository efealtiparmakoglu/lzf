//! Stage CLI output beside its destination, then publish only after success.

use std::fs::{self, File, OpenOptions};
use std::io;
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicU64, Ordering};

use crate::error::{Error, Result};

static NEXT_ID: AtomicU64 = AtomicU64::new(0);

pub(crate) struct PendingOutput {
    temporary: PathBuf,
    destination: PathBuf,
}

impl PendingOutput {
    pub(crate) fn create(input: &Path, destination: &Path, force: bool) -> Result<(Self, File)> {
        let source = fs::canonicalize(input)?;
        if let Ok(target) = fs::canonicalize(destination) {
            if source == target {
                return Err(Error::Usage(
                    "input and output must be different files".into(),
                ));
            }
        }
        #[cfg(unix)]
        {
            use std::os::unix::fs::MetadataExt;
            if let Ok(target) = fs::metadata(destination) {
                let source = fs::metadata(input)?;
                if source.dev() == target.dev() && source.ino() == target.ino() {
                    return Err(Error::Usage(
                        "input and output must be different files".into(),
                    ));
                }
            }
        }
        if fs::symlink_metadata(destination).is_ok() && !force {
            return Err(Error::Exists(destination.display().to_string()));
        }
        let parent = destination
            .parent()
            .filter(|p| !p.as_os_str().is_empty())
            .unwrap_or(Path::new("."));
        for _ in 0..128 {
            let id = NEXT_ID.fetch_add(1, Ordering::Relaxed);
            let temporary = parent.join(format!(".lzf-{}-{id}.tmp", std::process::id()));
            match OpenOptions::new()
                .write(true)
                .create_new(true)
                .open(&temporary)
            {
                Ok(file) => {
                    return Ok((
                        Self {
                            temporary,
                            destination: destination.to_path_buf(),
                        },
                        file,
                    ))
                }
                Err(e) if e.kind() == io::ErrorKind::AlreadyExists => continue,
                Err(e) => return Err(e.into()),
            }
        }
        Err(io::Error::new(
            io::ErrorKind::AlreadyExists,
            "could not create temporary output",
        )
        .into())
    }

    // The caller must flush and close the writer before publishing (also needed on Windows).
    pub(crate) fn commit(self, force: bool) -> Result<()> {
        if force {
            fs::rename(&self.temporary, &self.destination)?;
        } else {
            // Unlike an exists-check followed by rename, hard_link cannot replace a
            // destination that another process creates while compression is running.
            fs::hard_link(&self.temporary, &self.destination).map_err(|e| {
                if e.kind() == io::ErrorKind::AlreadyExists {
                    Error::Exists(self.destination.display().to_string())
                } else {
                    Error::Io(e)
                }
            })?;
        }
        Ok(())
    }
}

impl Drop for PendingOutput {
    fn drop(&mut self) {
        let _ = fs::remove_file(&self.temporary);
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::io::Write;

    #[test]
    fn concurrent_destination_is_not_replaced_without_force() {
        let dir = std::env::temp_dir().join(format!("lzf-publish-{}", std::process::id()));
        fs::create_dir_all(&dir).unwrap();
        let input = dir.join("input");
        let output = dir.join("output");
        fs::write(&input, b"source").unwrap();
        let (pending, mut file) = PendingOutput::create(&input, &output, false).unwrap();
        let temporary = pending.temporary.clone();
        file.write_all(b"new output").unwrap();
        file.flush().unwrap();
        drop(file);
        fs::write(&output, b"concurrent file").unwrap();
        assert!(matches!(pending.commit(false), Err(Error::Exists(_))));
        assert_eq!(fs::read(&output).unwrap(), b"concurrent file");
        assert!(!temporary.exists());
        fs::remove_dir_all(dir).unwrap();
    }
}
