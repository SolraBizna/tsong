mod config;
mod ffmpeg;
mod generation;
mod logical;
mod physical;
mod playback;
mod playlist;
mod prefs;
mod reference;
mod scan;
mod db;
mod ui;
mod remote;
mod errors;

use reference::Reference;
use generation::{GenerationTracker, GenerationValue, NOT_GENERATED};
use physical::{PhysicalFile, PhysicalFileRef, FileID};
use logical::{LogicalSong, LogicalSongRef, SongID};
use playlist::{Playlist, PlaylistRef, PlaylistID, Playmode};
use playback::{PlaybackCommand, PlaybackStatus};
use remote::{Remote, RemoteTarget};
use scan::ScanThread;
use log::error;

#[cfg(target_os = "linux")]
mod alsa;

fn main() {
    #[cfg(target_os = "linux")]
    alsa::suppress_logs();
    env_logger::init();
    std::thread::spawn(logical::maybe_write_example_import_script);
    match prefs::read() {
        Err(x) => error!("Error reading preferences: {}", x),
        Ok(_) => (),
    }
    db::open_database().unwrap();
    ffmpeg::init();
    ui::go();
}
