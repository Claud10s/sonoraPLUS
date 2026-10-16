use gpui::prelude::*;
use gpui::{App, Entity};
use i18n::t;
use state::Lyrics;
use ui::{ActiveTheme as _, Menu, MenuItem, Picker, Popovers, Scrollbar};

pub(crate) const LYRICS_SOURCES: &str = "lyrics-sources";

/// Shares the current source and available sheets across all lyrics surfaces.
/// Resolve the sheet again on click because incoming providers can reorder the answers.
pub(crate) fn lyrics_sources(
    lyrics: &Entity<Lyrics>,
    group: &Popovers,
    scrollbar: &Entity<Scrollbar>,
    cx: &App,
) -> Picker {
    let state = lyrics.read(cx);
    let sonora = state::Sonora::global(cx);
    let personal_source = state
        .following()
        .and_then(|id| {
            let provider = sonora.session.read(cx).slug_for(id)?;
            sonora
                .lyric_book
                .read(cx)
                .data
                .current(&state::lyric_track_key(provider, id))
                .map(|r| r.source.clone())
        })
        .unwrap_or_default();
    let personal_source = match personal_source.as_str() {
        "import" => t!("book-imported-source"),
        "editor" => t!("book-edited-source"),
        _ => personal_source.into(),
    };
    let label = state.current().map_or_else(
        || t!("lyrics-select-source"),
        |hit| match hit.source {
            music::lyrics::LOCAL => t!("lyrics-source-local"),
            state::PERSONAL_LYRICS => t!("book-personal-source", source = &personal_source),
            _ => t!("lyrics-source", source = hit.source),
        },
    );
    let following = state.following().map(str::to_owned);
    Picker::new(LYRICS_SOURCES, group, label)
        .tooltip("lyrics-select-source")
        .width(Picker::WIDE)
        .menu(
            Menu::new("lyrics-source-menu")
                .w(Picker::WIDE)
                .max_h(cx.theme().metrics.control * 7.)
                .scrollbar(scrollbar.clone()),
        )
        .when(state.hits().is_empty(), |picker| {
            picker.item(MenuItem::new("lyrics-no-sources", t!("lyrics-no-sources")).disabled())
        })
        .item(
            MenuItem::new("open-lyrics-monitor", t!("book-title")).on_click(|_, _, cx| {
                cx.dispatch_action(&input::OpenLyricsMonitor);
            }),
        )
        .items(state.hits().iter().enumerate().map(|(index, hit)| {
            let kind = match (hit.lyrics.worded(), hit.lyrics.synced()) {
                (true, _) => t!("lyrics-kind-words"),
                (_, true) => t!("lyrics-kind-lines"),
                _ => t!("lyrics-kind-plain"),
            };
            let source = match hit.source {
                music::lyrics::LOCAL => t!("lyrics-source-local"),
                state::PERSONAL_LYRICS => t!("book-personal-source", source = &personal_source),
                _ => hit.source.into(),
            };
            let label = t!("lyrics-source-option", source = &source, kind = &kind);
            let selected = state.current() == Some(hit);
            let hit = hit.clone();
            let lyrics = lyrics.clone();
            let following = following.clone();
            MenuItem::new((LYRICS_SOURCES, index), label)
                .checked(selected)
                .on_click(move |_, _, cx| {
                    lyrics.update(cx, |lyrics, cx| {
                        if lyrics.following() != following.as_deref() {
                            return;
                        }
                        if let Some(index) = lyrics.hits().iter().position(|found| found == &hit) {
                            lyrics.choose(index, cx);
                        }
                    });
                })
        }))
}
