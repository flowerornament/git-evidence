//! Where a streamed run's lines go: this store's notes, or a file another
//! store records.

use super::RecordError;
use crate::format::event::EventLine;

/// Where a run's lines go.
#[derive(Debug)]
pub(super) enum Destination {
    /// This store's notes.
    Store,
    /// One file holding the whole run so far, for another store to record: a
    /// measuring host that holds no evidence. Each write is checked against
    /// this store as [`Evidence::record_dry_run`](crate::Evidence::record_dry_run) would, then
    /// replaces the file whole, so the file is always a run the caller's store
    /// can record, and an interrupted run still returns the run it began.
    Returned {
        path: std::path::PathBuf,
        lines: Vec<EventLine>,
    },
}

/// Replaces `path` with `lines`, one per line, through a sibling temporary
/// file, so a reader never sees half a run.
pub(super) fn replace_file(path: &std::path::Path, lines: &[EventLine]) -> Result<(), RecordError> {
    let mut bytes = Vec::new();
    for line in lines {
        bytes.extend(serde_json::to_vec(line)?);
        bytes.push(b'\n');
    }
    let io = |source| RecordError::Returned {
        path: path.to_owned(),
        source,
    };
    if let Some(parent) = path.parent().filter(|p| !p.as_os_str().is_empty()) {
        std::fs::create_dir_all(parent).map_err(io)?;
    }
    let mut partial = path.as_os_str().to_owned();
    partial.push(".partial");
    // The bytes reach the disk before the rename publishes them, so a crash
    // leaves the old file or the whole new one, never a short one.
    let mut file = std::fs::File::create(&partial).map_err(io)?;
    std::io::Write::write_all(&mut file, &bytes).map_err(io)?;
    file.sync_all().map_err(io)?;
    std::fs::rename(&partial, path).map_err(io)
}
