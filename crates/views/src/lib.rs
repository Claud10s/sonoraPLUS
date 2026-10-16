mod chrome;
mod floating_lyrics;
mod lyrics_monitor;
mod root;
mod screens;
mod shared;
mod shells;

pub use floating_lyrics::FloatingLyrics;
pub use lyrics_monitor::LyricsMonitor;

#[cfg(target_os = "macos")]
pub use chrome::traffic_lights;
pub use root::Root;
use screens::artist::ArtistView;
use screens::detail::DetailView;
use screens::genre::GenreView;
use screens::history::HistoryView;
use screens::home::HomeView;
pub use screens::library::LibraryView;
pub use screens::login::LoginView;
pub use screens::settings::SettingsView;
use screens::song::SongView;
use screens::user::UserView;
use shared::adaptive::Adaptive;
use shells::fullscreen::FullscreenView;
