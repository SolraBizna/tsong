//! This module is in charge of recursively searching music directories for
//! songs, recognizing known song files and identifying unknown ones.

use anyhow::anyhow;
use std::{
    collections::VecDeque,
    ffi::OsStr,
    fs,
    path::{Path, PathBuf},
    rc::Rc,
    sync::{atomic::{AtomicU32, Ordering}, Arc, mpsc},
    thread,
};

use crate::*;

/// Encapsulates the communication channels to and from the search thread.
pub struct ScanThread {
    rescan_request_tx: mpsc::Sender<Vec<String>>,
    scan_result_rx: mpsc::Receiver<anyhow::Result<()>>,
    // Incremented by `rescan`. Decremented by the scan thread.
    // Oh boy we made it an arc...
    scans_left: Arc<AtomicU32>,
}

impl ScanThread {
    /// Starts a new search thread, ready to begin its work.
    pub fn new() -> ScanThread {
        let (rescan_request_tx, rescan_request_rx) = mpsc::channel();
        let (scan_result_tx, scan_result_rx) = mpsc::channel();
        let scans_left: Arc<AtomicU32> = Arc::new(0.into());
        let scans_left_clone = scans_left.clone();
        thread::Builder::new().name("song scan thread".to_owned())
            .spawn(move || search_thread_body(rescan_request_rx,
                                              scan_result_tx,
                                              scans_left_clone))
            .expect("Unable to spawn song scan thread");
        ScanThread { rescan_request_tx, scan_result_rx, scans_left }
    }
    /// Initiates a scan of the given music directories.
    pub fn rescan(&mut self, dirs: Vec<String>) -> anyhow::Result<()> {
        // set scanning to true BEFORE sending!
        self.scans_left.fetch_add(1, Ordering::SeqCst);
        self.rescan_request_tx.send(dirs)?;
        Ok(())
    }
    /// Returns a scan result, blocking if necessary. Returns:
    /// - `Err(...)` → The scanning thread crashed
    /// - `Ok(None)` → Scanning is complete
    /// - `Ok(Some(Ok(...)))` → A scan finished (but scanning is not
    ///   necessarily complete)
    /// - `Ok(Some(Err(...)))` → An error was encountered scanning a particular
    ///   file, but the scan is continuing
    pub fn get_result_blocking(&mut self)
    -> anyhow::Result<Option<anyhow::Result<()>>> {
        if self.scans_left.load(Ordering::SeqCst) == 0 { Ok(None) }
        else {
            // if we fetched it and it wasn't zero, then—since we are the
            // sole consumer of this queue—we will DEFINITELY get at least
            // one Ok(()) before scans_left becomes zero.
            match self.scan_result_rx.recv() {
                Ok(x) => Ok(Some(x)),
                Err(_) => Err(anyhow!("Scan thread crashed")),
            }
        }
    }
    /// Returns a scan result, without blocking. Returns:
    /// - `Err(...)` → The scanning thread crashed
    /// - `Ok((true, None))` → Scanning is complete
    /// - `Ok((false, None))` → Nothing to report right now
    /// - `Ok((false, Some(Ok(...))))` → A scan finished (but scanning is not
    ///   necessarily complete)
    /// - `Ok((false, Some(Err(...))))` → An error was encountered scanning a
    ///   particular file, but the scan is continuing onward.
    pub fn get_result_nonblocking(&mut self)
    -> anyhow::Result<(bool, Option<anyhow::Result<()>>)> {
        if self.scans_left.load(Ordering::SeqCst) == 0 { Ok((true, None)) }
        else {
            // if we fetched it and it wasn't zero, then—since we are the
            // sole consumer of this queue—we will DEFINITELY get at least
            // one Ok(()) before scans_left becomes zero.
            match self.scan_result_rx.try_recv() {
                Ok(x) => Ok((false, Some(x))),
                Err(mpsc::TryRecvError::Empty) => Ok((false, None)),
                Err(_) => Err(anyhow!("Scan thread crashed")),
            }
        }
    }
}

fn interrogate_file(ent: &fs::DirEntry, fs_metadata: &fs::Metadata,
                    size: u64, prefix: &Path)
    -> anyhow::Result<()> {
    let absolute_path = ent.path();
    let relative_path: String = absolute_path.strip_prefix(prefix).unwrap()
        .to_string_lossy().into_owned();
    let mtime = match fs_metadata.modified() {
        // Only returns an error if the local OS doesn't support mtimes. I
        // doubt Tsong would otherwise function on such an OS, but just in
        // case, use a special placeholder value here.
        Err(_) => 456,
        Ok(x) => x.duration_since(std::time::SystemTime::UNIX_EPOCH)?.as_secs(),
    };
    if let Some(_) = physical::saw_file(size, mtime,
                                        &relative_path, &absolute_path) {
        // It hasn't changed since the last time we saw it.
        return Ok(())
    }
    // Okay, so we don't believe we've seen this physical file before. We need
    // to open it, get metadata, checksum it, etc.
    let mut avf = ffmpeg::AVFormat::open_input(&absolute_path)?;
    avf.find_stream_info()?;
    let best_stream_id = match avf.find_best_stream()? {
        Some(x) => x,
        None => {
            // TODO: not a music file
            return Ok(())
        }
    };
    let metadata = avf.read_metadata(Some(best_stream_id));
    let duration = avf.estimate_duration(best_stream_id);
    // We've got the metadata from ffmpeg. We're pretty sure at this point that
    // it's a music file. (Or something we can play as one, at least.) Checksum
    // the whole file to get its file ID.
    let fileid = FileID::from_file(fs::File::open(&absolute_path)?)?;
    physical::scanned_file(&fileid, size, mtime, duration, &relative_path,
                           &absolute_path, metadata)?;
    // Everything went okay. We scanned the file. We got its metadata. It has
    // been added to our physical file database.
    Ok(())
}

fn search_thread_body(rescan_request_rx: mpsc::Receiver<Vec<String>>,
                      scan_result_tx: mpsc::Sender<anyhow::Result<()>>,
                      scans_left: Arc<AtomicU32>) {
    while let Ok(dir_list) = rescan_request_rx.recv() {
        let mut dir_queue: VecDeque<(PathBuf, Rc<PathBuf>)> = dir_list
            .into_iter().map(PathBuf::from).map(|x| {
                let y = x.clone();
                (x, Rc::new(y))
            }).collect();
        while let Some((dir, prefix)) = dir_queue.pop_back() {
            let read_dir_iterator = match fs::read_dir(&dir) {
                Ok(x) => x,
                Err(x) => {
                    let x = anyhow!(x)
                        .context(format!("While opening directory {:?}", dir));
                    match scan_result_tx.send(Err(x)) {
                        Ok(_) => (),
                        Err(_) => return, // we got dropped, oh well
                    }
                    continue
                },
            };
            for ent in read_dir_iterator {
                let ent = match ent {
                    Ok(x) => x,
                    Err(x) => {
                        let x = anyhow!(x)
                            .context(format!("While iterating directory {:?}",
                                             dir));
                        match scan_result_tx.send(Err(x)) {
                            Ok(_) => (),
                            Err(_) => return, // we got dropped, oh well
                        }
                        continue
                    },
                };
                match ent.path().file_name().map(OsStr::to_string_lossy) {
                    Some(x) => if x.starts_with(".") || x.ends_with("\r")
                        || x.ends_with(".xml") || x.ends_with(".itl")
                        || x.ends_with(".itdb") || x.ends_with(".m3u")
                        || x.ends_with(".itc")
                        || (x.starts_with("iTunes Library ")
                            && !x.contains(".")) {
                            continue
                    },
                    None => continue,
                }
                let metadata = match ent.path().metadata() {
                    Err(x) => {
                        let x = anyhow!(x)
                            .context(format!("While getting metadata for {:?}",
                                             ent.path()));
                        match scan_result_tx.send(Err(x)) {
                            Ok(_) => (),
                            Err(_) => return, // we got dropped, oh well
                        }
                        continue
                    },
                    Ok(x) => x,
                };
                let size = metadata.len();
                if metadata.file_type().is_dir() {
                    // TODO: check for loops
                    dir_queue.push_back((ent.path(),
                                         prefix.clone()));
                    continue
                }
                else {
                    match interrogate_file(&ent, &metadata, size, &prefix) {
                        Ok(_) => (),
                        Err(x) => {
                            let x = x.context(format!("While scanning {:?}",
                                                      ent.path()));
                            match scan_result_tx.send(Err(x)) {
                                Ok(_) => (),
                                Err(_) => return, // we got dropped, oh well
                            }
                            continue
                        },
                    }
                }
            }
        }
        scans_left.fetch_sub(1, Ordering::SeqCst);
        match scan_result_tx.send(Ok(())) {
            Ok(_) => (),
            Err(_) => return, // we got dropped, oh well
        }
    }
}
