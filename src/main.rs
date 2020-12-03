pub mod config;
pub mod prefs;
pub mod scan;
pub mod physical;
pub mod ffmpeg;

use lsx::sha256;

pub type FileID = [u8; sha256::HASHBYTES];

fn main() {
    prefs::read().unwrap();
    let mut scan_thread = scan::ScanThread::new();
    scan_thread.rescan(prefs::get_music_paths()).unwrap();
    scan_thread.rescan(prefs::get_music_paths()).unwrap();
    loop {
        match scan_thread.get_result_blocking() {
            Err(x) => { eprintln!("Scan terminated. {}", x); break; },
            Ok(None) => { eprintln!("All scans finished."); break; },
            Ok(Some(Ok(x))) => eprintln!("A scan finished. {:?}", x),
            Ok(Some(Err(x))) => eprintln!("Error during scan: {:?}", x),
        }
    }
}
