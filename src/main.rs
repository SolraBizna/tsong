pub mod config;
pub mod ffmpeg;
pub mod generation;
pub mod logical;
pub mod physical;
pub mod playback;
pub mod playlist;
pub mod prefs;
pub mod reference;
pub mod scan;
pub mod db;
pub mod ui;
pub mod remote;
pub mod errors;

pub use reference::Reference;
pub use generation::{GenerationTracker, GenerationValue, NOT_GENERATED};
pub use physical::{PhysicalFile, PhysicalFileRef, FileID};
pub use logical::{LogicalSong, LogicalSongRef, SongID};
pub use playlist::{Playlist, PlaylistRef, PlaylistID, Playmode};
pub use playback::{PlaybackCommand, PlaybackStatus};
pub use remote::{Remote, RemoteTarget};
pub use scan::ScanThread;

#[cfg(target_os = "linux")]
mod alsa;

fn main() {
    #[cfg(target_os = "linux")]
    alsa::suppress_logs();
    env_logger::init();
    std::thread::spawn(logical::maybe_write_example_import_script);
    prefs::read().unwrap();
    db::open_database().unwrap();
    ffmpeg::init();
    ui::go();
}
