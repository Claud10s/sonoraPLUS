use std::collections::{BTreeMap, BTreeSet};
use std::fs::File;
use std::io::{Read as _, Write as _};
use std::path::{Path, PathBuf};

use anyhow::{Result, ensure};
use flate2::{Compression, read::GzDecoder, write::GzEncoder};
use gpui::Context;
use serde::{Deserialize, Serialize};

use super::{BookData, LyricBook, Revision, store};

const FORMAT: &str = "sonora-lyrics";
const VERSION: u32 = 1;
const MAX_BYTES: u64 = 256 * 1024 * 1024;
const MAX_REVISIONS: usize = 50_000;

/// A portable, versioned JSON document compressed with gzip. No account or device data.
#[derive(Serialize, Deserialize)]
struct Archive {
    format: String,
    version: u32,
    created_at: u64,
    selected: BTreeMap<String, String>,
    revisions: Vec<Revision>,
}

impl LyricBook {
    /// Runs a user-requested archive operation between queued database mutations.
    pub fn backup(&mut self, path: PathBuf, restore: bool, cx: &mut Context<Self>) {
        if !self.ready || self.busy() {
            self.error = true;
            cx.notify();
            return;
        }
        self.backup_result = None;
        self.error = false;
        let path = match restore {
            true => path,
            false => path.join(format!(
                "Sonora-Lyrics-{}-{:08x}.json.gz",
                store::now(),
                fastrand::u32(..)
            )),
        };
        let store = self.store.clone();
        self.write = Some(cx.spawn(async move |this, cx| {
            let destination = path.clone();
            let result = cx
                .background_executor()
                .spawn(async move {
                    store.edit(|data| match restore {
                        true => import(data, &destination),
                        false => export(data, &destination),
                    })
                })
                .await;
            this.update(cx, |this, cx| {
                this.write = None;
                if result.is_ok() {
                    this.backup_result = Some((
                        match restore {
                            true => "book-backup-restored",
                            false => "book-backup-created",
                        },
                        path,
                    ));
                }
                this.apply(result, cx);
                this.next(cx);
            })
            .ok();
        }));
        cx.notify();
    }
}

/// Finishes the complete archive before publishing it; existing backup files are never replaced.
fn export(data: &mut BookData, path: &Path) -> Result<()> {
    ensure!(
        data.revisions.len() <= MAX_REVISIONS,
        "too many lyrics revisions"
    );
    let archive = Archive {
        format: FORMAT.into(),
        version: VERSION,
        created_at: store::now(),
        selected: data
            .tracks
            .iter()
            .filter_map(|(key, row)| row.selected.as_ref().map(|id| (key.clone(), id.clone())))
            .collect(),
        revisions: data.revisions.values().cloned().collect(),
    };
    let bytes = serde_json::to_vec(&archive)?;
    ensure!(
        bytes.len() as u64 <= MAX_BYTES,
        "lyrics archive is too large"
    );
    let folder = path
        .parent()
        .ok_or_else(|| anyhow::anyhow!("backup directory missing"))?;
    let mut temporary = tempfile::NamedTempFile::new_in(folder)?;
    {
        let mut encoder = GzEncoder::new(temporary.as_file_mut(), Compression::default());
        encoder.write_all(&bytes)?;
        encoder.finish()?;
    }
    temporary.as_file().sync_all()?;
    temporary.persist_noclobber(path)?;
    data.backup_path = Some(path.to_path_buf());
    data.backup_at = archive.created_at;
    data.backup_saved = data.revisions.keys().cloned().collect();
    Ok(())
}

/// Validates the full archive before merging. Reimporting never duplicates revisions.
fn import(data: &mut BookData, path: &Path) -> Result<()> {
    let file = File::open(path)?;
    ensure!(
        file.metadata()?.len() <= MAX_BYTES * 2,
        "compressed archive is too large"
    );
    let mut bytes = Vec::new();
    GzDecoder::new(file)
        .take(MAX_BYTES + 1)
        .read_to_end(&mut bytes)?;
    ensure!(
        bytes.len() as u64 <= MAX_BYTES,
        "expanded archive is too large"
    );
    let archive: Archive = serde_json::from_slice(&bytes)?;
    ensure!(
        archive.format == FORMAT && archive.version == VERSION,
        "unsupported lyrics archive"
    );
    ensure!(
        archive.revisions.len() <= MAX_REVISIONS,
        "too many lyrics revisions"
    );
    let by_id: BTreeMap<_, _> = archive.revisions.iter().map(|r| (&r.id, r)).collect();
    ensure!(
        by_id.len() == archive.revisions.len(),
        "duplicate revision in archive"
    );
    for revision in &archive.revisions {
        store::validate_revision(revision)?;
        ensure!(
            revision.parents.iter().all(|id| {
                by_id
                    .get(id)
                    .is_some_and(|parent| parent.key == revision.key && parent.id != revision.id)
            }),
            "incomplete revision ancestry"
        );
    }
    for (key, id) in &archive.selected {
        ensure!(
            by_id.get(id).is_some_and(|r| &r.key == key),
            "invalid selected revision"
        );
    }
    let previous: BTreeMap<_, _> = data
        .tracks
        .iter()
        .filter_map(|(key, row)| row.selected.as_ref().map(|id| (key.clone(), id.clone())))
        .collect();
    let saved: BTreeSet<_> = archive.revisions.iter().map(|r| r.id.clone()).collect();
    data.merge(archive.revisions)?;
    // Local choices win; a fresh installation recovers the choices recorded in the backup.
    for (key, id) in archive.selected.into_iter().chain(previous) {
        if let Some(row) = data.tracks.get_mut(&key) {
            row.selected = Some(id);
        }
    }
    data.backup_path = Some(path.to_path_buf());
    data.backup_at = archive.created_at;
    data.backup_saved = saved;
    Ok(())
}
