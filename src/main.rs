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

pub use reference::Reference;
pub use generation::{GenerationTracker, GenerationValue, NOT_GENERATED};
pub use physical::{PhysicalFile, PhysicalFileRef, FileID};
pub use logical::{LogicalSong, LogicalSongRef, SongID};
pub use playlist::{Playlist, PlaylistRef};

fn main() {
    prefs::read().unwrap();
    ffmpeg::init();
    let mut scan_thread = scan::ScanThread::new();
    scan_thread.rescan(prefs::get_music_paths()).unwrap();
    scan_thread.rescan(prefs::get_music_paths()).unwrap();
    let _playlist_all = playlist::add_playlist_from_db("All Songs".to_owned(),
                                                      "true".to_owned(),
                                                      vec![SongID::from_db(8)
                                                      ], vec![], vec![]);
    let _playlist_one = playlist::add_playlist_from_db("One Song".to_owned(),
                                                      "".to_owned(),
                                                      vec![SongID::from_db(1)
                                                      ], vec![], vec![]);
    let playlist_mck = playlist::add_playlist_from_db("McKennitt".to_owned(),
                                                      "artist:contains \
                                                       \"McKennitt\""
                                                      .to_owned(),
                                                      vec![], vec![],
                                                      vec![("title".to_owned(),true)]);
    loop {
        match scan_thread.get_result_blocking() {
            Err(x) => { eprintln!("Scan terminated. {}", x); break; },
            Ok(None) => { eprintln!("All scans finished."); break; },
            Ok(Some(Ok(x))) => eprintln!("A scan finished. {:?}", x),
            Ok(Some(Err(x))) => eprintln!("Error during scan: {:?}", x),
        }
    }
    playback::set_future_playlist(Some(playlist_mck));
    playback::start_playing_song(logical::get_song_by_song_id(
        SongID::from_db(26)).unwrap());
    std::thread::sleep(std::time::Duration::new(1200, 0));
    eprintln!("-------------\nPAUSE!");
    playback::send_playback_command(playback::PlaybackCommand::Stop);
    std::thread::sleep(std::time::Duration::new(5, 0));
}
