mod backup;
mod files;
mod store;

use std::collections::VecDeque;
use std::path::PathBuf;
use std::sync::Arc;
use std::time::{Duration, Instant};

use anyhow::Result;
use gpui::{Context, Entity, Task};
use music::{Lyrics as Sheet, LyricsHit, LyricsProvider, LyricsQuery, Track, TrackKey};

use crate::playback::PlaybackEvent;
use crate::{AppSettings, Io, Playback, PlaybackState, Session, join};
pub(crate) use store::BookStore;
pub use store::{BookData, BookTrack, Revision, parse_text, track_key};

pub const PERSONAL_LYRICS: &str = "Personal library";
type Edit = Box<dyn FnOnce(&mut BookData) -> Result<()> + Send>;

/// The permanent lyrics library and qualified listening counts for this installation.
/// All writes are serialized, so a sync cannot overwrite an edit made while it was running.
pub struct LyricBook {
    pub data: BookData,
    pub ready: bool,
    pub error: bool,
    pub searching: bool,
    pub search_results: Option<(String, Vec<LyricsHit>)>,
    pub backup_result: Option<(&'static str, PathBuf)>,
    pub generation: u64,
    pub(crate) store: BookStore,
    playback: Entity<Playback>,
    session: Entity<Session>,
    settings: Entity<AppSettings>,
    providers: Vec<Arc<dyn LyricsProvider>>,
    io: Io,
    pending: VecDeque<Edit>,
    write: Option<Task<()>>,
    search: Option<Task<()>>,
    timer: Option<Task<()>>,
    listening: Option<String>,
    elapsed: Duration,
    last_tick: Instant,
    last_position: Duration,
    counted: bool,
}

impl LyricBook {
    pub fn new(
        playback: Entity<Playback>,
        session: Entity<Session>,
        settings: Entity<AppSettings>,
        providers: Vec<Arc<dyn LyricsProvider>>,
        database: storage::Database,
        cx: &mut Context<Self>,
    ) -> Self {
        cx.subscribe(&playback, |this, _, event, cx| match event {
            PlaybackEvent::EndedPlayback => {
                this.listening = None;
                this.elapsed = Duration::ZERO;
                this.counted = false;
            }
            PlaybackEvent::Seeked => {
                this.last_position = this.playback.read(cx).position();
            }
            _ => {}
        })
        .detach();
        let mut book = Self {
            data: BookData::default(),
            ready: false,
            error: false,
            searching: false,
            search_results: None,
            backup_result: None,
            generation: 0,
            store: BookStore(database),
            playback,
            session,
            settings,
            providers,
            io: Io::global(cx),
            pending: VecDeque::new(),
            write: None,
            search: None,
            timer: None,
            listening: None,
            elapsed: Duration::ZERO,
            last_tick: Instant::now(),
            last_position: Duration::ZERO,
            counted: false,
        };
        let store = book.store.clone();
        book.write = Some(cx.spawn(async move |this, cx| {
            let result = cx
                .background_executor()
                .spawn(async move { store.migrate() })
                .await;
            this.update(cx, |this, cx| {
                this.write = None;
                this.apply(result, cx);
                this.next(cx);
            })
            .ok();
        }));
        book.timer = Some(cx.spawn(async move |this, cx| {
            loop {
                cx.background_executor().timer(Duration::from_secs(1)).await;
                if this
                    .update(cx, |this, cx| {
                        this.tick(cx);
                    })
                    .is_err()
                {
                    break;
                }
            }
        }));
        book
    }

    pub fn busy(&self) -> bool {
        self.write.is_some() || self.searching
    }

    pub fn current_key(&self, cx: &gpui::App) -> Option<String> {
        let track = self.playback.read(cx).track()?;
        let id = track.id.as_deref()?;
        Some(track_key(self.session.read(cx).slug_for(id)?, id))
    }

    pub fn remember(&mut self, provider: String, track: Track, cx: &mut Context<Self>) {
        self.enqueue(
            Box::new(move |data| {
                data.ensure_track(&provider, &track);
                Ok(())
            }),
            cx,
        );
    }

    /// Counts actual progressing playback, never a seek, pause or buffering interval.
    fn tick(&mut self, cx: &mut Context<Self>) {
        let now = Instant::now();
        let delta = now.duration_since(self.last_tick);
        self.last_tick = now;
        if !self.ready {
            return;
        }
        let playback = self.playback.read(cx);
        let Some(track) = playback.track().cloned() else {
            return;
        };
        let Some(id) = track.id.as_deref() else {
            return;
        };
        let Some(provider) = self.session.read(cx).slug_for(id).map(str::to_owned) else {
            return;
        };
        let key = track_key(&provider, id);
        let position = playback.position();
        let playing = *playback.state() == PlaybackState::Playing;
        if self.listening.as_ref() != Some(&key) {
            self.listening = Some(key.clone());
            self.elapsed = Duration::ZERO;
            self.counted = false;
            self.last_position = position;
            self.remember(provider.clone(), track.clone(), cx);
        }
        let advanced = position.saturating_sub(self.last_position);
        self.last_position = position;
        if !playing || delta > Duration::from_secs(3) || advanced > delta + Duration::from_secs(2) {
            return;
        }
        self.elapsed += advanced.min(delta);
        let threshold = match track.duration.is_zero() {
            true => Duration::from_secs(30),
            false => (track.duration / 2).min(Duration::from_secs(30)),
        };
        if self.counted || self.elapsed < threshold {
            return;
        }
        self.counted = true;
        self.enqueue(
            Box::new(move |data| {
                let row = data.ensure_track(&provider, &track);
                row.listens = row.listens.saturating_add(1);
                row.last_played = store::now();
                Ok(())
            }),
            cx,
        );
    }

    pub fn save(&mut self, key: String, sheet: Sheet, source: String, cx: &mut Context<Self>) {
        self.enqueue(
            Box::new(move |data| {
                let row = data
                    .tracks
                    .get(&key)
                    .cloned()
                    .ok_or_else(|| anyhow::anyhow!("unknown track"))?;
                data.save(&row.provider, &row.track, sheet, source, false)
            }),
            cx,
        );
    }

    /// Chooses the automatic file library and fills it from all retained revisions.
    pub fn set_folder(&mut self, folder: PathBuf, cx: &mut Context<Self>) {
        self.enqueue(
            Box::new(move |data| {
                data.lyrics_folder = Some(folder);
                data.folder_error = true;
                Ok(())
            }),
            cx,
        );
    }

    pub fn import(&mut self, key: String, path: PathBuf, cx: &mut Context<Self>) {
        self.enqueue(
            Box::new(move |data| {
                anyhow::ensure!(
                    std::fs::metadata(&path)?.len() <= store::MAX_IMPORT,
                    "lyrics file is too large"
                );
                let text = std::fs::read_to_string(path)?;
                let sheet = match serde_json::from_str::<Sheet>(&text) {
                    Ok(sheet) => sheet,
                    Err(_) => parse_text(&text)?,
                };
                let row = data
                    .tracks
                    .get(&key)
                    .cloned()
                    .ok_or_else(|| anyhow::anyhow!("unknown track"))?;
                data.save(&row.provider, &row.track, sheet, "import".into(), false)
            }),
            cx,
        );
    }

    /// Keeping a chosen revision creates a descendant of all conflicting heads.
    pub fn select_revision(&mut self, id: String, cx: &mut Context<Self>) {
        self.enqueue(
            Box::new(move |data| {
                let revision = data
                    .revisions
                    .get(&id)
                    .cloned()
                    .ok_or_else(|| anyhow::anyhow!("unknown revision"))?;
                data.save(
                    &revision.provider,
                    &revision.track,
                    revision.sheet,
                    revision.source,
                    true,
                )
            }),
            cx,
        );
    }

    pub fn link(&mut self, key: String, target: Option<String>, cx: &mut Context<Self>) {
        self.enqueue(
            Box::new(move |data| {
                let song = match target {
                    Some(target) => data
                        .tracks
                        .get(&target)
                        .ok_or_else(|| anyhow::anyhow!("unknown target"))?
                        .song
                        .clone(),
                    None => key.clone(),
                };
                data.tracks
                    .get_mut(&key)
                    .ok_or_else(|| anyhow::anyhow!("unknown track"))?
                    .song = song;
                Ok(())
            }),
            cx,
        );
    }

    /// Explicitly copies text or timing to another recording, never inferred from a title match.
    pub fn copy_from(
        &mut self,
        key: String,
        target: String,
        text_only: bool,
        cx: &mut Context<Self>,
    ) {
        self.enqueue(
            Box::new(move |data| {
                let revision = data
                    .current(&target)
                    .cloned()
                    .ok_or_else(|| anyhow::anyhow!("target has no lyrics"))?;
                let row = data
                    .tracks
                    .get(&key)
                    .cloned()
                    .ok_or_else(|| anyhow::anyhow!("unknown track"))?;
                let sheet = match (text_only, revision.sheet) {
                    (true, Sheet::Synced { lines }) => Sheet::plain(
                        lines
                            .iter()
                            .map(|l| l.text.as_str())
                            .collect::<Vec<_>>()
                            .join("\n"),
                    ),
                    (_, sheet) => sheet,
                };
                data.save(&row.provider, &row.track, sheet, revision.source, false)
            }),
            cx,
        );
    }

    pub fn search(&mut self, key: String, cx: &mut Context<Self>) {
        if self.searching {
            return;
        }
        let Some(row) = self.data.tracks.get(&key).cloned() else {
            return;
        };
        let settings = self.settings.read(cx);
        let providers: Vec<_> = self
            .providers
            .iter()
            .filter(|p| settings.lyrics_provider_enabled(p.name()))
            .filter(|p| {
                p.name() != music::lyrics::LOCAL
                    || music::is_local_id(row.track.id.as_deref().unwrap_or_default())
            })
            .filter(|p| {
                p.name() == music::lyrics::LOCAL
                    || !music::is_local_id(row.track.id.as_deref().unwrap_or_default())
                    || settings.lyrics_for_local_files()
            })
            .cloned()
            .collect();
        let slug = self
            .session
            .read(cx)
            .slug_for(row.track.id.as_deref().unwrap_or_default());
        let query = LyricsQuery {
            title: row.track.name.clone(),
            artist: row.track.artists.clone(),
            album: Some(row.track.album.clone()),
            duration: row.track.duration,
            track: slug.map(|provider| TrackKey {
                provider,
                id: row.track.id.clone().unwrap_or_default(),
            }),
        };
        self.searching = true;
        self.error = false;
        let worker = self.io.spawn(async move {
            let mut hits = Vec::new();
            let mut answered = 0;
            let total = providers.len();
            for provider in providers {
                if let Ok(Ok(mut found)) =
                    tokio::time::timeout(Duration::from_secs(30), provider.search(&query)).await
                {
                    answered += 1;
                    hits.append(&mut found);
                }
            }
            let instrumental = music::lyrics::instrumental(&query, &hits);
            let retained = retained_answers(&query, hits.clone());
            let hits = music::lyrics::rank(&query, hits);
            anyhow::ensure!(
                !hits.is_empty() || (total > 0 && answered == total),
                "lyrics lookup incomplete"
            );
            Ok((hits, retained, instrumental))
        });
        self.search = Some(cx.spawn(async move |this, cx| {
            let result = join(worker).await;
            this.update(cx, |this, cx| {
                this.searching = false;
                match result {
                    Ok((hits, retained, instrumental)) => {
                        this.search_results = Some((key, hits.clone()));
                        this.record_answers(row.provider, row.track, retained, instrumental, cx);
                    }
                    Err(error) => {
                        log::warn!("lyrics-library: search failed: {error:#}");
                        this.error = true;
                    }
                }
                cx.notify();
            })
            .ok();
        }));
        cx.notify();
    }

    pub(crate) fn record_answers(
        &mut self,
        provider: String,
        track: Track,
        hits: Vec<LyricsHit>,
        instrumental: bool,
        cx: &mut Context<Self>,
    ) {
        self.enqueue(
            Box::new(move |data| {
                let key = data.ensure_track(&provider, &track).key.clone();
                data.tracks.get_mut(&key).unwrap().checked = Some(
                    match (hits.is_empty(), instrumental) {
                        (false, _) => "found",
                        (_, true) => "instrumental",
                        _ => "missing",
                    }
                    .into(),
                );
                for hit in hits.into_iter().filter(|hit| !hit.lyrics.is_empty()) {
                    if data.revisions.values().any(|revision| {
                        revision.key == key
                            && revision.source == hit.source
                            && revision.sheet == hit.lyrics
                    }) {
                        continue;
                    }
                    if let Err(error) = store::validate_sheet(&hit.lyrics) {
                        log::warn!("lyrics-library: invalid provider answer: {error:#}");
                        continue;
                    }
                    let selected = data.tracks[&key].selected.clone();
                    data.save(&provider, &track, hit.lyrics, hit.source.into(), false)?;
                    if selected.is_some() {
                        data.tracks.get_mut(&key).unwrap().selected = selected;
                    }
                }
                Ok(())
            }),
            cx,
        );
    }

    pub fn export(&mut self, key: String, folder: PathBuf, cx: &mut Context<Self>) {
        self.enqueue(
            Box::new(move |data| {
                let revision = data
                    .current(&key)
                    .ok_or_else(|| anyhow::anyhow!("no saved lyrics"))?;
                let path = folder.join(format!("sonora-{}.json", revision.id));
                use std::io::Write as _;
                let mut file = std::fs::OpenOptions::new()
                    .write(true)
                    .create_new(true)
                    .open(path)?;
                file.write_all(&serde_json::to_vec_pretty(&revision.sheet)?)?;
                Ok(())
            }),
            cx,
        );
    }

    fn enqueue(&mut self, edit: Edit, cx: &mut Context<Self>) {
        self.pending.push_back(edit);
        self.next(cx);
    }

    fn next(&mut self, cx: &mut Context<Self>) {
        if self.write.is_some() {
            return;
        }
        let Some(edit) = self.pending.pop_front() else {
            return;
        };
        let store = self.store.clone();
        self.write = Some(cx.spawn(async move |this, cx| {
            let result = cx
                .background_executor()
                .spawn(async move { store.edit(edit) })
                .await;
            this.update(cx, |this, cx| {
                this.write = None;
                this.apply(result, cx);
                this.next(cx);
            })
            .ok();
        }));
        cx.notify();
    }

    fn apply(&mut self, result: Result<BookData>, cx: &mut Context<Self>) {
        match result {
            Ok(data) => {
                if self
                    .data
                    .tracks
                    .iter()
                    .any(|(k, v)| data.tracks.get(k).is_none_or(|r| r.selected != v.selected))
                    || self.data.revisions.len() != data.revisions.len()
                {
                    self.generation = self.generation.wrapping_add(1);
                }
                self.data = data;
                self.ready = true;
                self.error = false;
            }
            Err(error) => {
                log::warn!("lyrics-library: operation failed: {error:#}");
                self.error = true;
            }
        }
        cx.notify();
    }
}

/// Keeps the preferred answer first, then retains eligible originals from every service.
/// Display ranking deduplicates equivalent text; a personal file library must not do so.
pub(crate) fn retained_answers(query: &LyricsQuery, hits: Vec<LyricsHit>) -> Vec<LyricsHit> {
    let mut retained = music::lyrics::rank(query, hits.clone());
    music::lyrics::reshape(&mut retained);
    retained.extend(
        hits.into_iter()
            .filter(|hit| music::lyrics::eligible(query, hit)),
    );
    retained
}
