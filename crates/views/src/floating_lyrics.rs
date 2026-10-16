use gpui::prelude::*;
use gpui::{Context, Entity, MouseButton, Pixels, Size, Window, WindowControlArea, div, size};
use i18n::t;
use input::{CloseWindow, MinimizeWindow, ZoomWindow};
use state::{Lyrics, Playback, SideTab, Sonora};
use ui::{ActiveTheme as _, Button};

use crate::chrome::{Aside, LYRICS_SOURCES, lyrics_sources};

/// A separate compact lyrics surface sharing the main player's timing and sources.
pub struct FloatingLyrics {
    aside: Entity<Aside>,
    playback: Entity<Playback>,
    lyrics: Entity<Lyrics>,
    sources: ui::Popovers,
    source_scrollbar: Entity<ui::Scrollbar>,
    grabbed: bool,
    single_line: bool,
}

impl FloatingLyrics {
    pub fn new(cx: &mut Context<Self>) -> Self {
        let sonora = Sonora::global(cx);
        let playback = sonora.playback.clone();
        let queue = sonora.queue.clone();
        let settings = sonora.settings.clone();
        let lyrics = sonora.lyrics.clone();
        cx.observe(&playback, |_, _, cx| cx.notify()).detach();
        cx.observe(&queue, |_, _, cx| cx.notify()).detach();
        cx.observe(&settings, |_, _, cx| cx.notify()).detach();
        cx.observe(&lyrics, |_, _, cx| {
            cx.notify();
        })
        .detach();
        let aside = cx.new(|cx| {
            let mut aside = Aside::new(queue.clone(), playback.clone(), SideTab::Lyrics, cx);
            aside.compact(3);
            aside
        });
        Self {
            aside,
            playback,
            lyrics,
            sources: ui::Popovers::default(),
            source_scrollbar: cx.new(|_| ui::Scrollbar::new(gpui::ScrollHandle::new())),
            grabbed: false,
            single_line: false,
        }
    }

    /// A locked footprint: one header plus one or three lyric rows, scaled with the theme.
    pub fn window_size(theme: &ui::Theme, single_line: bool) -> Size<Pixels> {
        size(
            theme.metrics.control * 12.,
            theme.metrics.control_small * if single_line { 2. } else { 4. },
        )
    }
}

impl Render for FloatingLyrics {
    fn render(&mut self, window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let theme = *cx.theme();
        window.set_rem_size(theme.font_size);
        let mut footprint = Self::window_size(&theme, self.single_line);
        if self.sources.shows(LYRICS_SOURCES) {
            footprint.height += theme.metrics.control * 8.;
        }
        if window.viewport_size() != footprint {
            window.resize(footprint);
        }
        let title = self
            .playback
            .read(cx)
            .track()
            .map(|track| match track.artists.is_empty() {
                true => track.name.clone(),
                false => format!("{} - {}", track.artists, track.name),
            })
            .unwrap_or_else(|| t!("floating-lyrics-title").to_string());
        window.set_window_title(&title);

        let root = div()
            .relative()
            .flex()
            .flex_col()
            .size_full()
            .bg(theme.background)
            .text_color(theme.foreground)
            .font(crate::root::ui_font(
                Sonora::global(cx).settings.read(cx).font(),
            ))
            .on_action(|_: &CloseWindow, window, _| window.remove_window())
            .on_action(|_: &MinimizeWindow, window, _| window.minimize_window())
            .on_action(|_: &ZoomWindow, _, _| {})
            .child(
                div()
                    .flex()
                    .flex_none()
                    .items_center()
                    .px_3()
                    .h(theme.metrics.control_small)
                    .bg(theme.sidebar)
                    .text_color(theme.muted_foreground)
                    .text_size(theme.text(ui::Text::Small))
                    .child(
                        div()
                            .flex_1()
                            .min_w_0()
                            .truncate()
                            .child(title)
                            .window_control_area(WindowControlArea::Drag)
                            .on_mouse_down(
                                MouseButton::Left,
                                cx.listener(|this, _, _, _| this.grabbed = true),
                            )
                            .on_mouse_up(
                                MouseButton::Left,
                                cx.listener(|this, _, _, _| this.grabbed = false),
                            )
                            .on_mouse_down_out(cx.listener(|this, _, _, _| this.grabbed = false))
                            .on_mouse_move(cx.listener(|this, _, window, _| {
                                if std::mem::take(&mut this.grabbed) {
                                    window.start_window_move();
                                }
                            })),
                    )
                    .child(lyrics_sources(
                        &self.lyrics,
                        &self.sources,
                        &self.source_scrollbar,
                        cx,
                    ))
                    .child(
                        Button::new("floating-lyrics-lines")
                            .ghost()
                            .small()
                            .label(match self.single_line {
                                true => t!("floating-lyrics-three-lines"),
                                false => t!("floating-lyrics-one-line"),
                            })
                            .tooltip("floating-lyrics-lines")
                            .on_click(cx.listener(|this, _, window, cx| {
                                this.single_line = !this.single_line;
                                let rows = if this.single_line { 1 } else { 3 };
                                this.aside.update(cx, |aside, cx| {
                                    aside.compact(rows);
                                    cx.notify();
                                });
                                window.resize(Self::window_size(cx.theme(), this.single_line));
                                cx.notify();
                            })),
                    )
                    .child(
                        Button::new("close-floating-lyrics")
                            .ghost()
                            .small()
                            .icon("icons/x.svg")
                            .tooltip("floating-lyrics-close")
                            .on_click(|_, window, _| window.remove_window()),
                    ),
            )
            .child(
                div()
                    .flex_1()
                    .min_h_0()
                    .overflow_hidden()
                    .px_1()
                    .child(self.aside.clone()),
            );
        #[cfg(any(target_os = "linux", target_os = "freebsd"))]
        let root = root.child(ui::WindowFrame::new());
        root
    }
}
