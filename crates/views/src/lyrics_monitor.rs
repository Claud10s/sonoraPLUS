use std::time::Duration;

use gpui::prelude::*;
use gpui::{
    App, Bounds, Context, Div, Entity, PathPromptOptions, ScrollHandle, Task, Window, WindowBounds,
    WindowOptions, div, size,
};
use i18n::t;
use music::{Lyrics as Sheet, LyricsLine, LyricsWord, Voice};
use state::{BookTrack, LyricBook, Playback, Sonora};
use ui::{ActiveTheme as _, Button, Input, Scrollbar, Scroller, Text};

use crate::chrome::Chrome;

const PAGE: usize = 50;

/// One editable lyric line with optional individual word timestamps.
struct EditLine {
    text: Entity<Input>,
    time: Entity<Input>,
    original: Option<LyricsLine>,
    words: Vec<(String, Entity<Input>)>,
}

/// A dedicated workspace for durable lyrics, listening counts, revisions and cloud backup.
pub struct LyricsMonitor {
    focus: gpui::FocusHandle,
    book: Entity<LyricBook>,
    playback: Entity<Playback>,
    query: Entity<Input>,
    target: Option<String>,
    selected: Option<String>,
    lines: Vec<EditLine>,
    cursor: usize,
    word_cursor: Option<(usize, usize)>,
    dirty: bool,
    draft_version: u64,
    pending_save: Option<(String, Sheet, u64)>,
    loaded_revision: Option<String>,
    message: Option<&'static str>,
    filter: usize,
    page: usize,
    editor_page: usize,
    revision_page: usize,
    backups_open: bool,
    guide_open: bool,
    folder_open: bool,
    scroll: Entity<Scrollbar>,
    picker: Option<Task<()>>,
}

impl LyricsMonitor {
    pub fn open(cx: &mut App) {
        if let Some(window) = cx.windows().into_iter().find_map(|w| w.downcast::<Self>()) {
            window
                .update(cx, |_, window, _| window.activate_window())
                .ok();
            return;
        }
        let control = cx.theme().metrics.control;
        let bounds = Bounds::centered(None, size(control * 32., control * 24.), cx);
        if let Err(error) = cx.open_window(
            WindowOptions {
                window_bounds: Some(WindowBounds::Windowed(bounds)),
                window_min_size: Some(size(control * 18., control * 14.)),
                ..Default::default()
            },
            |window, cx| {
                let view = cx.new(Self::new);
                let focus = view.read(cx).focus.clone();
                window.focus(&focus, cx);
                let weak = view.downgrade();
                window.on_window_should_close(cx, move |_, cx| {
                    weak.update(cx, |view, cx| {
                        if view.dirty || view.pending_save.is_some() {
                            view.message = Some("book-unsaved");
                            cx.notify();
                            false
                        } else {
                            true
                        }
                    })
                    .unwrap_or(true)
                });
                view
            },
        ) {
            log::warn!("lyrics-library: cannot open monitor: {error:#}");
        }
    }

    fn new(cx: &mut Context<Self>) -> Self {
        let book = Sonora::global(cx).lyric_book.clone();
        let playback = Sonora::global(cx).playback.clone();
        cx.observe(&book, |this, book, cx| {
            if let Some((key, sheet, version)) = &this.pending_save {
                let book = book.read(cx);
                if book.data.current(key).is_some_and(|r| &r.sheet == sheet) {
                    if *version == this.draft_version {
                        this.dirty = false;
                    }
                    this.pending_save = None;
                    this.message = Some("book-saved");
                } else if book.error && !book.busy() {
                    this.pending_save = None;
                    this.message = Some("book-error");
                }
            }
            if !this.dirty {
                let revision = this
                    .selected
                    .as_ref()
                    .and_then(|key| book.read(cx).data.current(key))
                    .cloned();
                if let Some(revision) = revision
                    && this.loaded_revision.as_ref() != Some(&revision.id)
                {
                    this.loaded_revision = Some(revision.id);
                    this.load(revision.sheet, cx);
                }
            }
            cx.notify();
        })
        .detach();
        cx.observe(&playback, |_, _, cx| cx.notify()).detach();
        let chrome = Chrome::entity(cx);
        cx.observe(&chrome, |_, _, cx| cx.notify()).detach();
        let query = cx.new(|cx| Input::new("book-search", cx).clearable());
        cx.observe(&query, |this, _, cx| {
            this.page = 0;
            cx.notify();
        })
        .detach();
        let watcher = cx.entity_id();
        Self {
            focus: cx.focus_handle(),
            book,
            playback,
            query,
            target: None,
            selected: None,
            lines: vec![],
            cursor: 0,
            word_cursor: None,
            dirty: false,
            draft_version: 0,
            pending_save: None,
            loaded_revision: None,
            message: None,
            filter: 0,
            page: 0,
            editor_page: 0,
            revision_page: 0,
            backups_open: false,
            guide_open: false,
            folder_open: false,
            scroll: cx.new(|_| Scrollbar::new(ScrollHandle::new()).watching(watcher)),
            picker: None,
        }
    }

    fn select(&mut self, key: String, cx: &mut Context<Self>) {
        if self.dirty {
            self.message = Some("book-unsaved");
            cx.notify();
            return;
        }
        self.selected = Some(key.clone());
        self.loaded_revision = self.book.read(cx).data.current(&key).map(|r| r.id.clone());
        self.scroll
            .read(cx)
            .scroll()
            .set_offset(gpui::point(gpui::Pixels::ZERO, gpui::Pixels::ZERO));
        let sheet = self
            .book
            .read(cx)
            .data
            .current(&key)
            .map(|r| r.sheet.clone());
        self.load(sheet.unwrap_or_else(|| Sheet::plain("")), cx);
    }

    fn load(&mut self, sheet: Sheet, cx: &mut Context<Self>) {
        self.draft_version = self.draft_version.wrapping_add(1);
        let lines: Vec<(String, Option<LyricsLine>)> = match sheet {
            Sheet::Plain { text, .. } => text.lines().map(|s| (s.to_owned(), None)).collect(),
            Sheet::Synced { lines } => lines
                .iter()
                .map(|l| (l.text.clone(), Some(l.clone())))
                .collect(),
        };
        self.lines = lines
            .into_iter()
            .map(|(text, original)| {
                let time = original
                    .as_ref()
                    .map(|l| stamp(l.start))
                    .unwrap_or_default();
                let words = original
                    .as_ref()
                    .and_then(|l| l.words.as_ref())
                    .map(|words| {
                        words
                            .iter()
                            .map(|w| (w.text.clone(), self.field("book-time", stamp(w.start), cx)))
                            .collect()
                    })
                    .unwrap_or_default();
                EditLine {
                    text: self.field("book-line-text", text, cx),
                    time: self.field("book-time", time, cx),
                    original,
                    words,
                }
            })
            .collect();
        self.cursor = 0;
        self.word_cursor = None;
        self.editor_page = 0;
        self.revision_page = 0;
        self.dirty = false;
        self.message = None;
        cx.notify();
    }

    fn field(&self, hint: &'static str, value: String, cx: &mut Context<Self>) -> Entity<Input> {
        let mut previous = value.clone();
        let input = cx.new(|cx| {
            let mut input = Input::new(hint, cx).compact();
            input.set_text(value, cx);
            input
        });
        cx.observe(&input, move |this, input, cx| {
            let text = input.read(cx).text();
            if text == previous {
                return;
            }
            previous = text.to_owned();
            this.mark_dirty();
            cx.notify();
        })
        .detach();
        input
    }

    fn selected_track(&self, cx: &App) -> Option<BookTrack> {
        self.book
            .read(cx)
            .data
            .tracks
            .get(self.selected.as_ref()?)
            .cloned()
    }

    /// Invalidates a pending save's snapshot when the draft changes.
    fn mark_dirty(&mut self) {
        self.dirty = true;
        self.draft_version = self.draft_version.wrapping_add(1);
    }

    fn paste(&mut self, cx: &mut Context<Self>) {
        let Some(text) = cx.read_from_clipboard().and_then(|c| c.text()) else {
            return;
        };
        match state::parse_lyric_text(&text) {
            Ok(sheet) => {
                self.load(sheet, cx);
                self.mark_dirty();
            }
            Err(_) => self.message = Some("book-invalid"),
        }
        cx.notify();
    }

    fn choose_path(&mut self, mode: &'static str, cx: &mut Context<Self>) {
        if self.dirty && matches!(mode, "import" | "restore") {
            self.message = Some("book-unsaved");
            cx.notify();
            return;
        }
        let key = self.selected.clone();
        let receiver = cx.prompt_for_paths(PathPromptOptions {
            files: matches!(mode, "import" | "restore"),
            directories: !matches!(mode, "import" | "restore"),
            multiple: false,
            prompt: None,
        });
        self.picker = Some(cx.spawn(async move |this, cx| {
            let Ok(Ok(Some(paths))) = receiver.await else {
                return;
            };
            let Some(path) = paths.into_iter().next() else {
                return;
            };
            this.update(cx, |this, cx| {
                this.book.update(cx, |book, cx| match (mode, key) {
                    ("backup", _) => book.backup(path, false, cx),
                    ("restore", _) => book.backup(path, true, cx),
                    ("folder", _) => book.set_folder(path, cx),
                    ("import", Some(key)) => book.import(key, path, cx),
                    ("export", Some(key)) => book.export(key, path, cx),
                    _ => {}
                });
                this.message = (mode == "import").then_some("book-reload-after-import");
                cx.notify();
            })
            .ok();
        }));
    }

    fn save(&mut self, plain: bool, cx: &mut Context<Self>) {
        let Some(row) = self.selected_track(cx) else {
            return;
        };
        match self.sheet(plain, row.track.duration, cx) {
            Ok(sheet) => {
                self.pending_save = Some((row.key.clone(), sheet.clone(), self.draft_version));
                self.book.update(cx, |book, cx| {
                    book.save(row.key, sheet, "editor".into(), cx)
                });
                self.message = Some("book-saving");
            }
            Err(_) => self.message = Some("book-invalid-times"),
        }
        cx.notify();
    }

    /// A partially timed draft stays in the editor until every nonempty line has valid timing.
    fn sheet(&self, plain: bool, duration: Duration, cx: &App) -> anyhow::Result<Sheet> {
        let lines: Vec<_> = self
            .lines
            .iter()
            .filter(|l| !l.text.read(cx).text().trim().is_empty())
            .collect();
        anyhow::ensure!(!lines.is_empty(), "empty draft");
        if plain || lines.iter().all(|l| l.time.read(cx).text().is_empty()) {
            return Ok(Sheet::plain(
                lines
                    .iter()
                    .map(|l| l.text.read(cx).text())
                    .collect::<Vec<_>>()
                    .join("\n"),
            ));
        }
        let mut synced = Vec::new();
        for (index, line) in lines.iter().enumerate() {
            let start = parse_stamp(line.time.read(cx).text())?;
            if let Some(original) = &line.original
                && original.text == line.text.read(cx).text()
                && original.start == start
                && original.words.as_ref().map_or(0, Vec::len) == line.words.len()
                && original
                    .words
                    .iter()
                    .flatten()
                    .zip(&line.words)
                    .all(|(old, (_, time))| {
                        parse_stamp(time.read(cx).text()).is_ok_and(|at| at == old.start)
                    })
            {
                synced.push(original.clone());
                continue;
            }
            let end = match lines.get(index + 1) {
                Some(next) => parse_stamp(next.time.read(cx).text())?,
                None => duration.max(start),
            };
            anyhow::ensure!(end >= start, "timestamps out of order");
            let text = line.text.read(cx).text().to_owned();
            let mut result = line.original.clone().unwrap_or_else(|| LyricsLine {
                start,
                end: Some(end),
                text: text.clone(),
                romanized: None,
                words: None,
                secondary: vec![],
                voice: Voice::Lead,
            });
            if result.text != text {
                result.romanized = None;
                result.secondary.clear();
            }
            result.start = start;
            result.end = Some(end);
            result.text = text;
            if !line.words.is_empty() {
                let word_text = line
                    .words
                    .iter()
                    .map(|(text, _)| text.as_str())
                    .collect::<String>();
                anyhow::ensure!(
                    word_text.trim() == result.text.trim(),
                    "word text changed; rebuild words"
                );
                let mut words = Vec::new();
                for (i, (text, time)) in line.words.iter().enumerate() {
                    let at = parse_stamp(time.read(cx).text())?;
                    let until = match line.words.get(i + 1) {
                        Some((_, time)) => parse_stamp(time.read(cx).text())?,
                        None => end,
                    };
                    anyhow::ensure!(
                        at >= start && until >= at && until <= end,
                        "invalid word time"
                    );
                    words.push(LyricsWord {
                        start: at,
                        end: until,
                        text: text.clone(),
                    });
                }
                result.words = Some(words);
            }
            synced.push(result);
        }
        Ok(Sheet::Synced {
            lines: synced.into(),
        })
    }

    fn capture(&mut self, cx: &mut Context<Self>) {
        let Some(row) = self.selected_track(cx) else {
            return;
        };
        if self.book.read(cx).current_key(cx).as_ref() != Some(&row.key) {
            self.message = Some("book-play-selected");
            cx.notify();
            return;
        }
        let playback = self.playback.read(cx);
        let value = stamp(playback.position());
        if let Some((line, word)) = self.word_cursor {
            if let Some((_, input)) = self.lines.get(line).and_then(|l| l.words.get(word)) {
                input.update(cx, |input, cx| input.set_text(value.clone(), cx));
                if word == 0 {
                    self.lines[line]
                        .time
                        .update(cx, |input, cx| input.set_text(value, cx));
                }
                self.word_cursor = match word + 1 < self.lines[line].words.len() {
                    true => Some((line, word + 1)),
                    false => None,
                };
            }
        } else if let Some(line) = self.lines.get(self.cursor) {
            line.time.update(cx, |input, cx| input.set_text(value, cx));
            self.cursor += 1;
            self.editor_page = self.cursor.saturating_sub(1) / PAGE;
        }
        self.mark_dirty();
        cx.notify();
    }

    /// Moves the complete draft by a small offset without affecting another publication.
    fn shift(&mut self, milliseconds: i64, cx: &mut Context<Self>) {
        let shift = |at: Duration| match milliseconds >= 0 {
            true => at.saturating_add(Duration::from_millis(milliseconds as u64)),
            false => at.saturating_sub(Duration::from_millis(milliseconds.unsigned_abs())),
        };
        for line in &mut self.lines {
            for time in std::iter::once(&line.time).chain(line.words.iter().map(|(_, time)| time)) {
                if let Ok(at) = parse_stamp(time.read(cx).text()) {
                    time.update(cx, |input, cx| input.set_text(stamp(shift(at)), cx));
                }
            }
            if let Some(original) = &mut line.original {
                for lane in &mut original.secondary {
                    lane.start = shift(lane.start);
                    lane.end = lane.end.map(shift);
                    if let Some(words) = &mut lane.words {
                        for word in words {
                            word.start = shift(word.start);
                            word.end = shift(word.end);
                        }
                    }
                }
            }
        }
        self.mark_dirty();
        cx.notify();
    }

    fn prepare_words(&mut self, line: usize, cx: &mut Context<Self>) {
        let text = self.lines[line].text.read(cx).text().to_owned();
        let words = text
            .split_inclusive(char::is_whitespace)
            .filter(|s| !s.is_empty())
            .map(|word| (word.to_owned(), self.field("book-time", String::new(), cx)))
            .collect();
        self.lines[line].words = words;
        self.word_cursor = Some((line, 0));
        self.mark_dirty();
        cx.notify();
    }

    fn library(&self, cx: &mut Context<Self>) -> Div {
        let theme = *cx.theme();
        let book = self.book.read(cx);
        let query = self.query.read(cx).text().to_lowercase();
        let mut rows: Vec<_> = book
            .data
            .tracks
            .values()
            .filter(|row| {
                let current = book.data.current(&row.key);
                let matches = format!("{} {} {}", row.track.name, row.track.artists, row.provider)
                    .to_lowercase()
                    .contains(&query);
                matches
                    && match self.filter {
                        1 => current.is_none(),
                        2 => current.is_some_and(|r| !r.sheet.synced()),
                        3 => book.data.heads(&row.key).len() > 1,
                        _ => true,
                    }
            })
            .cloned()
            .collect();
        rows.sort_by(|a, b| {
            b.listens
                .cmp(&a.listens)
                .then_with(|| b.last_played.cmp(&a.last_played))
                .then_with(|| a.key.cmp(&b.key))
        });
        let total = rows.len();
        div()
            .flex()
            .flex_col()
            .gap_2()
            .child(
                div()
                    .flex()
                    .flex_wrap()
                    .gap_2()
                    .items_center()
                    .child(div().flex_1().min_w_0().child(self.query.clone()))
                    .child(
                        button(
                            "book-filter",
                            ["book-all", "book-missing", "book-plain", "book-conflicts"]
                                [self.filter],
                        )
                        .on_click(cx.listener(|this, _, _, cx| {
                            this.filter = (this.filter + 1) % 4;
                            this.page = 0;
                            cx.notify();
                        })),
                    )
                    .child(
                        button("book-prev", "book-previous")
                            .disabled(self.page == 0)
                            .on_click(cx.listener(|this, _, _, cx| {
                                this.page = this.page.saturating_sub(1);
                                cx.notify();
                            })),
                    )
                    .child(
                        button("book-next", "book-next")
                            .disabled((self.page + 1) * PAGE >= total)
                            .on_click(cx.listener(|this, _, _, cx| {
                                this.page += 1;
                                cx.notify();
                            })),
                    ),
            )
            .child(
                div()
                    .text_color(theme.muted_foreground)
                    .child(t!("book-count", count = total)),
            )
            .children(
                rows.into_iter()
                    .skip(self.page * PAGE)
                    .take(PAGE)
                    .map(|row| {
                        let key = row.key.clone();
                        let target = row.key.clone();
                        let current = book.data.current(&row.key);
                        let kind = current.map(|r| kind(&r.sheet)).unwrap_or(
                            match row.checked.as_deref() {
                                Some("missing") => "book-not-found",
                                Some("instrumental") => "lyrics-instrumental",
                                _ => "book-unchecked",
                            },
                        );
                        let played = date(row.last_played);
                        let group_total: u64 = book
                            .data
                            .tracks
                            .values()
                            .filter(|r| r.song == row.song)
                            .map(|r| r.listens)
                            .sum();
                        let saved = current.is_some();
                        let sync = current.is_some_and(|r| book.data.backup_saved.contains(&r.id));
                        div()
                            .flex()
                            .flex_wrap()
                            .items_center()
                            .gap_2()
                            .p_2()
                            .border_b_1()
                            .border_color(theme.border)
                            .child(
                                div()
                                    .flex_1()
                                    .min_w_0()
                                    .flex()
                                    .flex_col()
                                    .child(format!("{} — {}", row.track.artists, row.track.name))
                                    .child(
                                        div()
                                            .text_size(theme.text(Text::Small))
                                            .text_color(theme.muted_foreground)
                                            .child(t!(
                                                "book-row",
                                                provider = row.provider.as_str(),
                                                count = row.listens,
                                                total = group_total,
                                                previous = row.previous
                                            )),
                                    )
                                    .child(
                                        div()
                                            .text_size(theme.text(Text::Small))
                                            .child(t!("book-last-played", time = &played)),
                                    ),
                            )
                            .child(i18n::lookup(kind, None))
                            .child(i18n::lookup(
                                if sync {
                                    "book-backed-up"
                                } else if saved {
                                    "book-local"
                                } else {
                                    "book-not-saved"
                                },
                                None,
                            ))
                            .child(
                                button(
                                    gpui::SharedString::from(format!("book-open-{key}")),
                                    "book-edit",
                                )
                                .on_click(
                                    cx.listener(move |this, _, _, cx| this.select(key.clone(), cx)),
                                ),
                            )
                            .child(
                                button(
                                    gpui::SharedString::from(format!("book-target-{target}")),
                                    "book-use-reference",
                                )
                                .on_click(cx.listener(
                                    move |this, _, _, cx| {
                                        this.target = Some(target.clone());
                                        cx.notify();
                                    },
                                )),
                            )
                    }),
            )
    }

    fn editor(&self, cx: &mut Context<Self>) -> Div {
        let theme = *cx.theme();
        let Some(row) = self.selected_track(cx) else {
            return div().child(t!("book-select-song"));
        };
        let key = row.key.clone();
        let search_key = key.clone();
        let play = row.track.clone();
        let duration = ui::runtime(row.track.duration);
        let book = self.book.read(cx);
        let current = book.data.current(&key);
        let position = stamp(self.playback.read(cx).position());
        let mut revisions: Vec<_> = book
            .data
            .revisions
            .values()
            .filter(|r| r.key == key)
            .collect();
        revisions.sort_by(|a, b| b.created.cmp(&a.created).then_with(|| a.id.cmp(&b.id)));
        let version_count = revisions.len();
        div()
            .flex()
            .flex_col()
            .gap_3()
            .pt_4()
            .child(
                div()
                    .text_size(theme.text(Text::Large))
                    .child(format!("{} — {}", row.track.artists, row.track.name)),
            )
            .child(format!(
                "{} · {} · {}",
                row.provider, row.track.album, duration
            ))
            .when_some(current, |panel, revision| {
                panel.child(t!("book-origin", source = revision.source.as_str()))
            })
            .child(
                div()
                    .flex()
                    .flex_wrap()
                    .gap_2()
                    .child(button("book-play", "book-play").on_click(cx.listener(
                        move |this, _, _, cx| this.playback.update(cx, |p, cx| p.play(&play, cx)),
                    )))
                    .child(button("book-toggle", "book-pause").on_click(cx.listener(
                        |this, _, _, cx| this.playback.update(cx, |p, cx| p.toggle_play(cx)),
                    )))
                    .child(
                        button("book-provider-search", "book-find")
                            .disabled(book.searching)
                            .on_click(cx.listener(move |this, _, _, cx| {
                                this.book
                                    .update(cx, |book, cx| book.search(search_key.clone(), cx))
                            })),
                    )
                    .child(
                        button("book-import", "book-import")
                            .on_click(cx.listener(|this, _, _, cx| this.choose_path("import", cx))),
                    )
                    .child(
                        button("book-export", "book-export")
                            .on_click(cx.listener(|this, _, _, cx| this.choose_path("export", cx))),
                    )
                    .child(
                        button("book-paste", "book-paste")
                            .on_click(cx.listener(|this, _, _, cx| this.paste(cx))),
                    )
                    .child(button("book-reload", "book-discard").on_click(cx.listener(
                        |this, _, _, cx| {
                            this.dirty = false;
                            if let Some(key) = this.selected.clone() {
                                this.select(key, cx);
                            }
                        },
                    ))),
            )
            .when_some(
                book.search_results
                    .as_ref()
                    .filter(|(searched, _)| searched == &key),
                |panel, (_, hits)| {
                    panel.child(div().flex().flex_wrap().gap_2().children(
                        hits.iter().enumerate().map(|(index, hit)| {
                            let sheet = hit.lyrics.clone();
                            let kind_label = i18n::lookup(kind(&hit.lyrics), None);
                            let label = t!(
                                "lyrics-source-option",
                                source = hit.source,
                                kind = &kind_label
                            );
                            Button::new(("book-found", index))
                                .small()
                                .label(label)
                                .on_click(cx.listener(move |this, _, _, cx| {
                                    if this.dirty {
                                        this.message = Some("book-unsaved");
                                    } else {
                                        this.load(sheet.clone(), cx);
                                        this.mark_dirty();
                                    }
                                    cx.notify();
                                }))
                        }),
                    ))
                },
            )
            .when_some(
                self.target
                    .as_ref()
                    .and_then(|target| book.data.tracks.get(target))
                    .filter(|target| target.key != key),
                |panel, target| {
                    let target_key = target.key.clone();
                    let link_key = key.clone();
                    let copy_key = key.clone();
                    let copy_target = target.key.clone();
                    let timed_key = key.clone();
                    let timed_target = target.key.clone();
                    panel.child(
                        div()
                            .flex()
                            .flex_col()
                            .gap_2()
                            .child(t!(
                                "book-reference",
                                title = target.track.name.as_str(),
                                provider = target.provider.as_str()
                            ))
                            .child(
                                div()
                                    .flex()
                                    .flex_wrap()
                                    .gap_2()
                                    .child(button("book-link", "book-link").on_click(cx.listener(
                                        move |this, _, _, cx| {
                                            this.book.update(cx, |book, cx| {
                                                book.link(
                                                    link_key.clone(),
                                                    Some(target_key.clone()),
                                                    cx,
                                                )
                                            })
                                        },
                                    )))
                                    .child(
                                        button("book-copy-text", "book-copy-text")
                                            .disabled(self.dirty)
                                            .on_click(cx.listener(move |this, _, _, cx| {
                                                this.book.update(cx, |book, cx| {
                                                    book.copy_from(
                                                        copy_key.clone(),
                                                        copy_target.clone(),
                                                        true,
                                                        cx,
                                                    )
                                                })
                                            })),
                                    )
                                    .child(
                                        button("book-copy-timing", "book-copy-timing")
                                            .disabled(self.dirty)
                                            .on_click(cx.listener(move |this, _, _, cx| {
                                                this.book.update(cx, |book, cx| {
                                                    book.copy_from(
                                                        timed_key.clone(),
                                                        timed_target.clone(),
                                                        false,
                                                        cx,
                                                    )
                                                })
                                            })),
                                    ),
                            ),
                    )
                },
            )
            .when(row.song != row.key, |panel| {
                let key = key.clone();
                panel.child(button("book-unlink", "book-unlink").on_click(cx.listener(
                    move |this, _, _, cx| {
                        this.book
                            .update(cx, |book, cx| book.link(key.clone(), None, cx))
                    },
                )))
            })
            .child(
                div()
                    .flex()
                    .flex_wrap()
                    .gap_2()
                    .child(button("book-stamp", "book-stamp").on_click(cx.listener(
                        |this, _, window, cx| {
                            window.focus(&this.focus, cx);
                            this.capture(cx);
                        },
                    )))
                    .child(
                        button("book-earlier", "book-earlier")
                            .on_click(cx.listener(|this, _, _, cx| this.shift(-100, cx))),
                    )
                    .child(
                        button("book-later", "book-later")
                            .on_click(cx.listener(|this, _, _, cx| this.shift(100, cx))),
                    )
                    .child(
                        button("book-restart-stamps", "book-restart-stamps").on_click(cx.listener(
                            |this, _, _, cx| {
                                this.cursor = 0;
                                this.word_cursor = None;
                                cx.notify();
                            },
                        )),
                    )
                    .child(
                        button("book-add-line", "book-add-line").on_click(cx.listener(
                            |this, _, _, cx| {
                                let text = this.field("book-line-text", String::new(), cx);
                                let time = this.field("book-time", String::new(), cx);
                                this.lines.push(EditLine {
                                    text,
                                    time,
                                    original: None,
                                    words: vec![],
                                });
                                this.mark_dirty();
                                this.editor_page = (this.lines.len() - 1) / PAGE;
                                cx.notify();
                            },
                        )),
                    )
                    .child(
                        button("book-save", "book-save")
                            .disabled(book.busy())
                            .on_click(cx.listener(|this, _, _, cx| this.save(false, cx))),
                    )
                    .child(
                        button("book-save-plain", "book-save-plain")
                            .disabled(book.busy())
                            .on_click(cx.listener(|this, _, _, cx| this.save(true, cx))),
                    ),
            )
            .child(t!(
                "book-capture-position",
                line = self.cursor + 1,
                time = &position
            ))
            .children(
                self.lines
                    .iter()
                    .enumerate()
                    .skip(self.editor_page * PAGE)
                    .take(PAGE)
                    .map(|(index, line)| {
                        div()
                            .flex()
                            .flex_col()
                            .gap_1()
                            .child(
                                div()
                                    .flex()
                                    .items_center()
                                    .gap_2()
                                    .child((index + 1).to_string())
                                    .child(
                                        div()
                                            .w(theme.metrics.control * 4.)
                                            .child(line.time.clone()),
                                    )
                                    .child(div().flex_1().min_w_0().child(line.text.clone()))
                                    .child(button(("book-words", index), "book-words").on_click(
                                        cx.listener(move |this, _, _, cx| {
                                            this.prepare_words(index, cx)
                                        }),
                                    ))
                                    .child(
                                        button(("book-remove-line", index), "book-remove-line")
                                            .on_click(cx.listener(move |this, _, _, cx| {
                                                this.lines.remove(index);
                                                this.word_cursor = None;
                                                this.cursor = this.cursor.min(this.lines.len());
                                                this.mark_dirty();
                                                cx.notify();
                                            })),
                                    ),
                            )
                            .children(line.words.iter().enumerate().map(|(word, (text, time))| {
                                div()
                                    .flex()
                                    .gap_2()
                                    .items_center()
                                    .pl_4()
                                    .child(div().w(theme.metrics.control * 4.).child(time.clone()))
                                    .child(text.clone())
                                    .child(
                                        button(
                                            ("book-word-capture", index * 10000 + word),
                                            "book-capture-word",
                                        )
                                        .on_click(
                                            cx.listener(move |this, _, _, cx| {
                                                this.word_cursor = Some((index, word));
                                                this.capture(cx);
                                            }),
                                        ),
                                    )
                            }))
                    }),
            )
            .child(
                div()
                    .flex()
                    .gap_2()
                    .child(
                        button("book-editor-prev", "book-previous")
                            .disabled(self.editor_page == 0)
                            .on_click(cx.listener(|this, _, _, cx| {
                                this.editor_page = this.editor_page.saturating_sub(1);
                                cx.notify();
                            })),
                    )
                    .child(
                        button("book-editor-next", "book-next")
                            .disabled((self.editor_page + 1) * PAGE >= self.lines.len())
                            .on_click(cx.listener(|this, _, _, cx| {
                                this.editor_page += 1;
                                cx.notify();
                            })),
                    ),
            )
            .child(t!("book-versions", count = version_count))
            .child(
                div()
                    .flex()
                    .gap_2()
                    .child(
                        button("book-versions-prev", "book-previous")
                            .disabled(self.revision_page == 0)
                            .on_click(cx.listener(|this, _, _, cx| {
                                this.revision_page = this.revision_page.saturating_sub(1);
                                cx.notify();
                            })),
                    )
                    .child(
                        button("book-versions-next", "book-next")
                            .disabled((self.revision_page + 1) * PAGE >= version_count)
                            .on_click(cx.listener(|this, _, _, cx| {
                                this.revision_page += 1;
                                cx.notify();
                            })),
                    ),
            )
            .when(book.data.heads(&key).len() > 1, |panel| {
                panel.child(t!("book-conflict-help"))
            })
            .children(
                revisions
                    .into_iter()
                    .skip(self.revision_page * PAGE)
                    .take(PAGE)
                    .map(|revision| {
                        let id = revision.id.clone();
                        let created = date(revision.created);
                        let kind_label = i18n::lookup(kind(&revision.sheet), None);
                        let label = t!(
                            "book-version",
                            source = revision.source.as_str(),
                            time = &created,
                            kind = &kind_label
                        );
                        Button::new(gpui::SharedString::from(format!(
                            "book-revision-{}",
                            revision.id
                        )))
                        .small()
                        .label(label)
                        .disabled(self.dirty)
                        .on_click(cx.listener(move |this, _, _, cx| {
                            this.book
                                .update(cx, |book, cx| book.select_revision(id.clone(), cx));
                            this.message = Some("book-reload-after-import");
                            cx.notify();
                        }))
                    }),
            )
    }

    /// Lets the user choose the automatic file library or retry an unavailable folder.
    fn folder(&self, cx: &mut Context<Self>) -> Div {
        let book = self.book.read(cx);
        let folder = book.data.lyrics_folder.clone();
        div()
            .flex()
            .flex_col()
            .gap_2()
            .when_some(folder.clone(), |panel, path| {
                let path = path.display().to_string();
                panel.child(t!("book-folder-path", path = &path))
            })
            .child(
                div()
                    .flex()
                    .flex_wrap()
                    .gap_2()
                    .child(
                        button("book-folder-choose", "book-folder-choose")
                            .disabled(!book.ready || book.busy())
                            .on_click(cx.listener(|this, _, _, cx| this.choose_path("folder", cx))),
                    )
                    .when_some(folder, |row, path| {
                        row.child(
                            button("book-folder-refresh", "book-folder-refresh")
                                .disabled(!book.ready || book.busy())
                                .on_click(cx.listener(move |this, _, _, cx| {
                                    this.book
                                        .update(cx, |book, cx| book.set_folder(path.clone(), cx));
                                })),
                        )
                    }),
            )
    }

    fn backups(&self, cx: &mut Context<Self>) -> Div {
        let book = self.book.read(cx);
        div()
            .flex()
            .flex_col()
            .gap_2()
            .when_some(book.data.backup_path.as_ref(), |panel, path| {
                let path = path.display().to_string();
                let time = date(book.data.backup_at);
                panel.child(t!("book-backup-last", path = &path, time = &time))
            })
            .when_some(book.backup_result.as_ref(), |panel, (key, path)| {
                panel
                    .child(i18n::lookup(key, None))
                    .child(path.display().to_string())
            })
            .child(
                div()
                    .flex()
                    .flex_wrap()
                    .gap_2()
                    .child(
                        button("book-backup", "book-backup")
                            .disabled(!book.ready || book.busy() || self.dirty)
                            .on_click(cx.listener(|this, _, _, cx| this.choose_path("backup", cx))),
                    )
                    .child(
                        button("book-restore", "book-restore")
                            .disabled(!book.ready || book.busy() || self.dirty)
                            .on_click(
                                cx.listener(|this, _, _, cx| this.choose_path("restore", cx)),
                            ),
                    ),
            )
    }
}

impl Render for LyricsMonitor {
    fn render(&mut self, window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let theme = *cx.theme();
        window.set_rem_size(theme.font_size);
        window.set_window_title(&t!("book-title"));
        self.scroll.read(cx).sync();
        let book = self.book.read(cx);
        let ready = book.ready;
        let busy = book.busy();
        let failed = book.error;
        let folder_failed = book.data.folder_error;
        div()
            .size_full()
            .key_context("LyricsMonitor")
            .track_focus(&self.focus)
            .on_action(cx.listener(|this, _: &input::CaptureLyricTime, _, cx| this.capture(cx)))
            .flex()
            .flex_col()
            .bg(theme.background)
            .text_color(theme.foreground)
            .font(crate::root::ui_font(
                Sonora::global(cx).settings.read(cx).font(),
            ))
            .child(
                div()
                    .flex()
                    .flex_wrap()
                    .gap_2()
                    .p_3()
                    .items_center()
                    .child(div().flex_1().child(t!("book-title")))
                    .when(self.selected.is_some(), |row| {
                        row.child(
                            button("book-close-editor", "book-close-editor")
                                .disabled(self.dirty)
                                .on_click(cx.listener(|this, _, _, cx| {
                                    this.selected = None;
                                    this.lines.clear();
                                    cx.notify();
                                })),
                        )
                    })
                    .child(
                        button("book-now", "book-now")
                            .disabled(self.dirty)
                            .on_click(cx.listener(|this, _, _, cx| {
                                let key = this.book.read(cx).current_key(cx);
                                if let Some(key) = key {
                                    this.select(key, cx);
                                }
                            })),
                    )
                    .child(button("book-cloud", "book-cloud").on_click(cx.listener(
                        |this, _, _, cx| {
                            this.backups_open = !this.backups_open;
                            cx.notify();
                        },
                    )))
                    .child(
                        button("book-folder", "book-folder")
                            .selected(self.folder_open)
                            .on_click(cx.listener(|this, _, _, cx| {
                                this.folder_open = !this.folder_open;
                                cx.notify();
                            })),
                    )
                    .child(
                        button("book-guide", "book-guide")
                            .icon("icons/info.svg")
                            .tooltip("book-guide")
                            .selected(self.guide_open)
                            .on_click(cx.listener(|this, _, _, cx| {
                                this.guide_open = !this.guide_open;
                                cx.notify();
                            })),
                    ),
            )
            .when(!ready, |panel| panel.child(t!("book-loading")))
            .when(busy, |panel| panel.child(t!("book-working")))
            .when(failed, |panel| panel.child(t!("book-error")))
            .when(folder_failed, |panel| panel.child(t!("book-folder-error")))
            .when_some(self.message, |panel, key| {
                panel.child(i18n::lookup(key, None))
            })
            .child(
                Scroller::new("lyrics-monitor-scroll", &self.scroll)
                    .child(
                        div()
                            .flex()
                            .flex_col()
                            .gap_3()
                            .p_3()
                            .when(self.guide_open, |panel| {
                                panel.child(
                                    div()
                                        .flex()
                                        .flex_col()
                                        .gap_3()
                                        .p_3()
                                        .child(t!("book-guide"))
                                        .child(t!("book-count-rule"))
                                        .child(t!("book-editor-help"))
                                        .child(t!("book-folder-help"))
                                        .child(t!("book-cloud-help")),
                                )
                            })
                            .when(self.backups_open, |panel| panel.child(self.backups(cx)))
                            .when(self.folder_open, |panel| panel.child(self.folder(cx)))
                            .when(self.selected.is_some(), |panel| {
                                panel.child(self.editor(cx))
                            })
                            .child(self.library(cx)),
                    )
                    .flex_1()
                    .min_h_0(),
            )
    }
}

fn button(id: impl Into<gpui::ElementId>, label: &'static str) -> Button {
    Button::new(id).small().label(i18n::lookup(label, None))
}
fn kind(sheet: &Sheet) -> &'static str {
    match (sheet.worded(), sheet.synced()) {
        (true, _) => "lyrics-kind-words",
        (_, true) => "lyrics-kind-lines",
        _ => "lyrics-kind-plain",
    }
}
fn stamp(at: Duration) -> String {
    format!(
        "{:02}:{:02}.{:03}",
        at.as_secs() / 60,
        at.as_secs() % 60,
        at.subsec_millis()
    )
}
fn parse_stamp(text: &str) -> anyhow::Result<Duration> {
    music::lyrics::lrc::stamp_of(text.trim()).ok_or_else(|| anyhow::anyhow!("invalid timestamp"))
}

fn date(seconds: u64) -> gpui::SharedString {
    if seconds == 0 {
        return t!("book-never");
    }
    crate::shared::cells::relative_stamp(i64::try_from(seconds).ok())
}
