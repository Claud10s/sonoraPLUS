use std::collections::{BTreeMap, BTreeSet};
use std::path::PathBuf;
use std::time::{Duration, SystemTime, UNIX_EPOCH};

use anyhow::{Context as _, Result, ensure};
use music::{Lyrics, Track};
use rusqlite::{OptionalExtension as _, params};
use serde::{Deserialize, Serialize};
use sha2::{Digest as _, Sha256};

const STATE_KEY: &str = "personal-lyrics-v1";
pub const MAX_IMPORT: u64 = 8 * 1024 * 1024;

/// One exact platform publication. Linking songs never merges their timing or counters.
#[derive(Clone, Serialize, Deserialize)]
pub struct BookTrack {
    pub key: String,
    pub provider: String,
    pub track: Track,
    pub listens: u64,
    pub previous: u64,
    pub last_played: u64,
    pub checked: Option<String>,
    pub selected: Option<String>,
    pub song: String,
}

/// An immutable revision. Parents identify conflicts without relying on device clocks.
#[derive(Clone, Serialize, Deserialize)]
pub struct Revision {
    pub id: String,
    pub key: String,
    pub provider: String,
    pub track: Track,
    pub source: String,
    pub sheet: Lyrics,
    pub parents: Vec<String>,
    pub created: u64,
}

#[derive(Clone, Default, Serialize, Deserialize)]
pub struct BookData {
    pub tracks: BTreeMap<String, BookTrack>,
    pub revisions: BTreeMap<String, Revision>,
    #[serde(default)]
    pub lyrics_folder: Option<PathBuf>,
    #[serde(default)]
    pub folder_error: bool,
    #[serde(default)]
    pub backup_path: Option<PathBuf>,
    #[serde(default)]
    pub backup_at: u64,
    #[serde(default)]
    pub backup_saved: BTreeSet<String>,
}

impl BookData {
    pub fn ensure_track(&mut self, provider: &str, track: &Track) -> &mut BookTrack {
        let key = track_key(provider, track.id.as_deref().unwrap_or_default());
        let row = self.tracks.entry(key.clone()).or_insert_with(|| BookTrack {
            song: key.clone(),
            key,
            provider: provider.to_owned(),
            track: track.clone(),
            listens: 0,
            previous: 0,
            last_played: 0,
            checked: None,
            selected: None,
        });
        row.track = track.clone();
        row
    }

    pub fn current(&self, key: &str) -> Option<&Revision> {
        self.tracks
            .get(key)?
            .selected
            .as_ref()
            .and_then(|id| self.revisions.get(id))
    }

    pub fn heads(&self, key: &str) -> Vec<&Revision> {
        let parents: BTreeSet<&str> = self
            .revisions
            .values()
            .filter(|r| r.key == key)
            .flat_map(|r| r.parents.iter().map(String::as_str))
            .collect();
        self.revisions
            .values()
            .filter(|r| r.key == key && !parents.contains(r.id.as_str()))
            .collect()
    }

    /// Saves a new version while retaining every earlier text and timing for recovery.
    pub fn save(
        &mut self,
        provider: &str,
        track: &Track,
        sheet: Lyrics,
        source: String,
        resolve: bool,
    ) -> Result<()> {
        validate_sheet(&sheet)?;
        let key = self.ensure_track(provider, track).key.clone();
        let parents = match resolve {
            true => self.heads(&key).iter().map(|r| r.id.clone()).collect(),
            false => self
                .current(&key)
                .map(|r| vec![r.id.clone()])
                .unwrap_or_default(),
        };
        let mut revision = Revision {
            id: String::new(),
            key: key.clone(),
            provider: provider.to_owned(),
            track: track.clone(),
            source,
            sheet,
            parents,
            created: now(),
        };
        revision.id = digest(&serde_json::to_vec(&revision)?);
        self.tracks.get_mut(&key).unwrap().selected = Some(revision.id.clone());
        self.revisions.insert(revision.id.clone(), revision);
        Ok(())
    }

    /// Imports revisions without overwriting a local selection when remote edits diverge.
    pub fn merge(&mut self, incoming: Vec<Revision>) -> Result<()> {
        for revision in &incoming {
            validate_revision(revision)?;
        }
        let affected: BTreeSet<String> = incoming.iter().map(|r| r.key.clone()).collect();
        for revision in incoming {
            if !self.tracks.contains_key(&revision.key) {
                self.ensure_track(&revision.provider, &revision.track);
            }
            self.revisions
                .entry(revision.id.clone())
                .or_insert(revision);
        }
        for key in affected {
            let heads = self.heads(&key);
            if heads.len() == 1 {
                let id = heads[0].id.clone();
                self.tracks.get_mut(&key).unwrap().selected = Some(id);
            } else if self.tracks[&key].selected.is_none() {
                let id = heads.first().map(|r| r.id.clone());
                self.tracks.get_mut(&key).unwrap().selected = id;
            }
        }
        Ok(())
    }
}

/// Durable user content, separate from the disposable provider cache.
#[derive(Clone)]
pub struct BookStore(pub storage::Database);

impl BookStore {
    pub fn read(&self) -> Result<BookData> {
        let connection = self.0.open()?;
        let json: Option<String> = connection
            .query_row(
                "SELECT value FROM app_state WHERE key = ?",
                [STATE_KEY],
                |r| r.get(0),
            )
            .optional()?;
        match json {
            Some(json) => serde_json::from_str(&json).context("cannot decode personal lyrics"),
            None => Ok(BookData::default()),
        }
    }

    /// Applies one queued mutation atomically; a failed import leaves the old library intact.
    pub fn edit(&self, edit: impl FnOnce(&mut BookData) -> Result<()>) -> Result<BookData> {
        let mut connection = self.0.open()?;
        let transaction = connection.transaction()?;
        let json: Option<String> = transaction
            .query_row(
                "SELECT value FROM app_state WHERE key = ?",
                [STATE_KEY],
                |r| r.get(0),
            )
            .optional()?;
        let mut data = json
            .map(|json| serde_json::from_str::<BookData>(&json))
            .transpose()?
            .unwrap_or_default();
        let old_folder = data.lyrics_folder.clone();
        let old_count = data.revisions.len();
        edit(&mut data)?;
        if data.folder_error
            || old_folder != data.lyrics_folder
            || old_count != data.revisions.len()
        {
            data.folder_error = match super::files::save(&data) {
                Ok(()) => false,
                Err(error) => {
                    log::warn!("lyrics-library: cannot write library files: {error:#}");
                    true
                }
            };
        }
        transaction.execute("INSERT INTO app_state(key,value) VALUES (?,?) ON CONFLICT(key) DO UPDATE SET value=excluded.value", params![STATE_KEY, serde_json::to_string(&data)?])?;
        transaction.commit()?;
        Ok(data)
    }

    /// Seeds only the records still retained by the old history, explicitly as previous starts.
    pub fn migrate(&self) -> Result<BookData> {
        let connection = self.0.open()?;
        let exists: bool = connection.query_row(
            "SELECT EXISTS(SELECT 1 FROM app_state WHERE key = ?)",
            [STATE_KEY],
            |r| r.get(0),
        )?;
        if exists {
            return self.edit(|data| {
                if data.lyrics_folder.is_none() {
                    data.lyrics_folder = dirs::document_dir().map(|path| path.join("SonoraLetras"));
                }
                data.folder_error = true;
                Ok(())
            });
        }
        let mut query = connection.prepare("SELECT provider, track_id, name, artists, album, duration_ms, explicit, MAX(played_at), COUNT(*) FROM plays GROUP BY provider, track_id")?;
        let rows = query
            .query_map([], |r| {
                Ok((
                    r.get::<_, String>(0)?,
                    Track {
                        id: Some(r.get(1)?),
                        name: r.get(2)?,
                        artists: r.get(3)?,
                        album: r.get(4)?,
                        duration: Duration::from_millis(r.get::<_, i64>(5)?.max(0) as u64),
                        explicit: r.get(6)?,
                        playable: true,
                        artist_refs: vec![],
                        album_id: None,
                        cover: None,
                        added_at: None,
                        added_by: None,
                        playcount: None,
                        popularity: 0,
                        track_number: 0,
                        disc_number: 0,
                        tags: vec![],
                        languages: vec![],
                        credits: vec![],
                    },
                    r.get::<_, i64>(7)?.max(0) as u64,
                    r.get::<_, u64>(8)?,
                ))
            })?
            .collect::<rusqlite::Result<Vec<_>>>()?;
        self.edit(|data| {
            data.lyrics_folder = dirs::document_dir().map(|path| path.join("SonoraLetras"));
            for (provider, track, time, count) in rows {
                let row = data.ensure_track(&provider, &track);
                row.previous = count;
                row.last_played = time / 1000;
            }
            Ok(())
        })
    }
}

pub fn parse_text(text: &str) -> Result<Lyrics> {
    ensure!(text.len() as u64 <= MAX_IMPORT, "lyrics file is too large");
    let text = text.trim_start_matches('\u{feff}');
    let lines = music::lyrics::lrc::parse(text);
    let sheet = match lines.is_empty() {
        true => Lyrics::plain(text),
        false => Lyrics::Synced {
            lines: lines.into(),
        },
    };
    validate_sheet(&sheet)?;
    Ok(sheet)
}

pub fn validate_sheet(sheet: &Lyrics) -> Result<()> {
    ensure!(!sheet.is_empty(), "lyrics are empty");
    ensure!(
        serde_json::to_vec(sheet)?.len() as u64 <= MAX_IMPORT,
        "lyrics are too large"
    );
    if let Lyrics::Synced { lines } = sheet {
        ensure!(
            lines.windows(2).all(|w| w[0].start <= w[1].start),
            "line times must be ordered"
        );
        for line in lines.iter() {
            ensure!(
                line.end.is_none_or(|end| end >= line.start),
                "line ends before it starts"
            );
            if let Some(words) = &line.words {
                ensure!(
                    words.windows(2).all(|w| w[0].start <= w[1].start),
                    "word times must be ordered"
                );
                ensure!(
                    words
                        .iter()
                        .all(|w| w.end >= w.start && w.start >= line.start),
                    "invalid word timing"
                );
            }
        }
    }
    Ok(())
}

pub fn validate_revision(revision: &Revision) -> Result<()> {
    ensure!(
        revision.key
            == track_key(
                &revision.provider,
                revision.track.id.as_deref().unwrap_or_default()
            ),
        "revision track mismatch"
    );
    ensure!(
        revision.id.len() == 64 && revision.id.bytes().all(|c| c.is_ascii_hexdigit()),
        "invalid revision id"
    );
    let mut copy = revision.clone();
    copy.id.clear();
    ensure!(
        digest(&serde_json::to_vec(&copy)?) == revision.id,
        "revision checksum mismatch"
    );
    validate_sheet(&revision.sheet)
}

pub fn track_key(provider: &str, id: &str) -> String {
    format!("{provider}:{id}")
}
pub fn now() -> u64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap_or_default()
        .as_secs()
}
fn digest(bytes: &[u8]) -> String {
    format!("{:x}", Sha256::digest(bytes))
}
