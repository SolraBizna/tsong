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

pub use reference::Reference;
pub use generation::{GenerationTracker, GenerationValue, NOT_GENERATED};
pub use physical::{PhysicalFile, PhysicalFileRef, FileID};
pub use logical::{LogicalSong, LogicalSongRef, SongID};
pub use playlist::{Playlist, PlaylistRef};
pub use playback::{PlaybackCommand};

fn main() {
    prefs::read().unwrap();
    db::load_database().unwrap();
    ffmpeg::init();
    scan::rescan();
    ui::go();
}
