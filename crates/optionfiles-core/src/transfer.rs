use std::fs::{self, File, OpenOptions};
use std::io::{Read, Write};
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};

use anyhow::{Context, Result, bail};

use crate::app::{remove_path, unique_path};
use crate::clipboard::ClipboardMode;

#[derive(Debug, Clone)]
pub struct TransferRequest {
    pub sources: Vec<PathBuf>,
    pub destination: PathBuf,
    pub mode: ClipboardMode,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ConflictAction {
    Overwrite,
    Skip,
    KeepBoth,
}

#[derive(Debug, Clone, Copy)]
pub struct ConflictResolution {
    pub action: ConflictAction,
    pub apply_to_batch: bool,
}

#[derive(Debug, Clone)]
pub struct TransferConflict {
    pub source: PathBuf,
    pub target: PathBuf,
}

#[derive(Debug, Clone, Default)]
pub struct TransferProgress {
    pub items_done: usize,
    pub items_total: usize,
    pub bytes_done: u64,
    pub current: Option<PathBuf>,
}

#[derive(Debug, Clone)]
pub struct TransferError {
    pub source: PathBuf,
    pub target: PathBuf,
    pub message: String,
}

#[derive(Debug, Clone, Default)]
pub struct TransferReport {
    pub completed: Vec<PathBuf>,
    pub skipped: Vec<PathBuf>,
    pub errors: Vec<TransferError>,
    pub cancelled: bool,
}


fn check_cancel(cancel: &AtomicBool) -> Result<()> {
    if cancel.load(Ordering::Acquire) {
        bail!("transfer cancelled");
    }
    Ok(())
}

fn validate_target(source: &Path, target: &Path) -> Result<()> {
    let metadata = source
        .symlink_metadata()
        .with_context(|| format!("cannot read {}", source.display()))?;
    let source_parent = source
        .parent()
        .context("cannot transfer filesystem root")?
        .canonicalize()?;
    let source_path = source_parent.join(source.file_name().context("invalid source name")?);
    let target_parent = target
        .parent()
        .context("invalid destination")?
        .canonicalize()?;
    let target_path = target_parent.join(target.file_name().context("invalid target name")?);
    if source_path == target_path {
        // Copy-to-self is resolved by KeepBoth, never by overwriting the source.
        return Ok(());
    }
    if metadata.is_dir() && target_parent.starts_with(source.canonicalize()?) {
        bail!("cannot copy a directory into itself or its descendant");
    }
    if target
        .symlink_metadata()
        .is_ok_and(|metadata| metadata.is_dir())
        && source_path.starts_with(target.canonicalize()?)
    {
        bail!("cannot overwrite a source ancestor");
    }
    if metadata.is_file() && target.symlink_metadata().is_ok() {
        #[cfg(unix)]
        {
            use std::os::unix::fs::MetadataExt;
            if let Ok(other) = fs::metadata(target) {
                if metadata.dev() == other.dev() && metadata.ino() == other.ino() {
                    bail!("source and target refer to the same file");
                }
            }
        }
    }
    Ok(())
}

static NEXT_STAGE: AtomicU64 = AtomicU64::new(0);

fn reserve_stage(target: &Path) -> Result<PathBuf> {
    let parent = target.parent().context("invalid target")?;
    let mut builder = fs::DirBuilder::new();
    #[cfg(unix)]
    {
        use std::os::unix::fs::DirBuilderExt;
        builder.mode(0o700);
    }
    loop {
        let id = NEXT_STAGE.fetch_add(1, Ordering::Relaxed);
        let stage = parent.join(format!(".optionfiles-transfer-{}-{id}", std::process::id()));
        match builder.create(&stage) {
            Ok(()) => return Ok(stage),
            Err(error) if error.kind() == std::io::ErrorKind::AlreadyExists => continue,
            Err(error) => return Err(error.into()),
        }
    }
}

fn copy_transaction(
    source: &Path,
    target: &Path,
    cancel: &AtomicBool,
    state: &mut TransferProgress,
    progress: &mut impl FnMut(&TransferProgress),
) -> Result<()> {
    validate_target(source, target)?;
    let source_path = source
        .parent()
        .context("invalid source")?
        .canonicalize()?
        .join(source.file_name().context("invalid source")?);
    let target_path = target
        .parent()
        .context("invalid target")?
        .canonicalize()?
        .join(target.file_name().context("invalid target")?);
    if source_path == target_path {
        bail!("cannot overwrite source with itself");
    }
    let stage = reserve_stage(target)?;
    let payload = stage.join("payload");
    let backup = stage.join("previous");
    let result = (|| {
        copy_node(source, &payload, cancel, state, progress)?;
        check_cancel(cancel)?;
        let exists = target.symlink_metadata().is_ok();
        if exists {
            fs::rename(target, &backup).context("cannot preserve existing target")?;
        }
        if let Err(error) = fs::rename(&payload, target) {
            if exists {
                fs::rename(&backup, target).with_context(|| {
                    format!(
                        "cannot restore target; previous data remains at {}",
                        backup.display()
                    )
                })?;
            }
            return Err(error.into());
        }
        Ok(())
    })();
    // Never discard the backup if publication failed and rollback failed too.
    if result.is_ok() || backup.symlink_metadata().is_err() {
        let _ = fs::remove_dir_all(&stage);
    }
    result
}

fn move_transaction(source: &Path, target: &Path, cancel: &AtomicBool) -> Result<bool> {
    validate_target(source, target)?;
    let source_path = source
        .parent()
        .context("invalid source")?
        .canonicalize()?
        .join(source.file_name().context("invalid source")?);
    let target_path = target
        .parent()
        .context("invalid target")?
        .canonicalize()?
        .join(target.file_name().context("invalid target")?);
    if source_path == target_path {
        bail!("cannot overwrite source with itself");
    }
    check_cancel(cancel)?;
    let stage = reserve_stage(target)?;
    let backup = stage.join("previous");
    let result = (|| {
        let exists = target.symlink_metadata().is_ok();
        if exists {
            fs::rename(target, &backup).context("cannot preserve existing target")?;
        }
        let moved = match fs::rename(source, target) {
            Ok(()) => return Ok(true),
            Err(error) if error.kind() == std::io::ErrorKind::CrossesDevices => false,
            Err(error) => {
                if exists {
                    fs::rename(&backup, target).with_context(|| {
                        format!(
                            "cannot restore target; previous data remains at {}",
                            backup.display()
                        )
                    })?;
                }
                return Err(error.into());
            }
        };
        if exists {
            fs::rename(&backup, target).with_context(|| {
                format!(
                    "cannot restore target; previous data remains at {}",
                    backup.display()
                )
            })?;
        }
        Ok(moved)
    })();
    if result.is_ok() || backup.symlink_metadata().is_err() {
        let _ = fs::remove_dir_all(stage);
    }
    result
}

fn copy_node(
    source: &Path,
    target: &Path,
    cancel: &AtomicBool,
    state: &mut TransferProgress,
    progress: &mut impl FnMut(&TransferProgress),
) -> Result<()> {
    check_cancel(cancel)?;
    state.current = Some(source.to_path_buf());
    progress(state);
    check_cancel(cancel)?;
    let metadata = source.symlink_metadata()?;
    if metadata.file_type().is_symlink() {
        let link = fs::read_link(source)?;
        #[cfg(unix)]
        std::os::unix::fs::symlink(link, target)?;
        #[cfg(windows)]
        if source.is_dir() {
            std::os::windows::fs::symlink_dir(link, target)?;
        } else {
            std::os::windows::fs::symlink_file(link, target)?;
        }
    } else if metadata.is_dir() {
        fs::create_dir(target)?;
        for child in fs::read_dir(source)? {
            let child = child?;
            copy_node(
                &child.path(),
                &target.join(child.file_name()),
                cancel,
                state,
                progress,
            )?;
        }
        fs::set_permissions(target, metadata.permissions())?;
    } else if metadata.is_file() {
        let mut input = File::open(source)?;
        let mut output = OpenOptions::new()
            .write(true)
            .create_new(true)
            .open(target)?;
        let mut buffer = [0u8; 64 * 1024];
        loop {
            check_cancel(cancel)?;
            let count = input.read(&mut buffer)?;
            if count == 0 {
                break;
            }
            output.write_all(&buffer[..count])?;
            state.bytes_done += count as u64;
            progress(state);
        }
        output.sync_all()?;
        fs::set_permissions(target, metadata.permissions())?;
    } else {
        bail!("unsupported file type: {}", source.display());
    }
    Ok(())
}

pub(crate) fn copy_to_path(source: &Path, target: &Path) -> Result<()> {
    copy_transaction(
        source,
        target,
        &AtomicBool::new(false),
        &mut TransferProgress::default(),
        &mut |_| {},
    )
}
