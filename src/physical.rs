//! This module handles *physical files*.
//!
//! This corresponds to the `physical_files` and `last_seens` tables of the
//! backing database.

use crate::*;
use lazy_static::lazy_static;

use std::{
    borrow::Cow,
    collections::{BTreeMap, HashMap},
    ffi::OsStr,
    fmt,
    fmt::{Debug, Display, Formatter},
    io,
    io::Read,
    path::{Path, PathBuf},
    sync::RwLock,
};
use anyhow::anyhow;

use lsx::{
    sha256,
    sha256::BufSha256,
};

use logical::SimilarityRec;

pub type PhysicalFileRef = Reference<PhysicalFile>;

/// A *physical file* has a unique identifier. That identifier is its SHA-256
/// hash.
#[derive(Clone,Copy,PartialEq,Eq,PartialOrd,Ord,Hash)]
pub struct FileID {
    inner: [u8; sha256::HASHBYTES],
}

impl Display for FileID {
    fn fmt(&self, fmt: &mut Formatter<'_>) -> fmt::Result {
        for b in &self.inner[..] {
            fmt.write_fmt(format_args!("{:02x}", b))?;
        }
        Ok(())
    }
}

impl Debug for FileID {
    fn fmt(&self, fmt: &mut Formatter<'_>) -> fmt::Result {
        Display::fmt(self, fmt)
    }
}

impl FileID {
    pub fn from_hash(bytes: &[u8; sha256::HASHBYTES]) -> FileID {
        FileID { inner: *bytes }
    }
    pub fn from_file<R: Read>(mut file: R) -> io::Result<FileID> {
        let mut hasher = BufSha256::new();
        let mut buf = [0u8; 16384];
        loop {
            match file.read(&mut buf[..])? {
                0 => break,
                x => hasher.update(&buf[..x]),
            }
        }
        Ok(FileID { inner: hasher.finish(&[]) })
    }
}

/// A *physical file* is a file on the disk. It contains (from our perspective)
/// exactly one *logical song*. Different encodings, etc. of the same logical
/// song correspond to different physical files.
#[derive(Debug)]
pub struct PhysicalFile {
    /// File's SHA-256 hash.
    id: FileID,
    /// File's size. Used to help ID a file during a scan.
    size: u64,
    /// File's (approximate) duration, in seconds. Used to help ID a file
    /// during a scan.
    duration: u32,
    /// All relative paths under which we've ever seen this file. Used to
    /// help quickly locate a known logical song before the scan finishes, and
    /// as a shortcut (in combination with size) to prevent having to rescan
    /// every file on every startup.
    relative_paths: Vec<String>,
    /// All absolute paths under which we've seen this file since startup. Used
    /// to actually find the file when it's time to play.
    absolute_paths: Vec<PathBuf>,
    /// Raw metadata, exactly as returned by FFMPEG.
    raw_meta: BTreeMap<String,String>,
}
    
lazy_static! {
    // Deadlock avoidance lexical order:
    // - `PHYSICAL_FILES` lock
    // - `FILES_BY_RELATIVE_PATH` lock
    // - Any given `PhysicalFile` lock (one at a time)
    static ref PHYSICAL_FILES
        : RwLock<HashMap<FileID, PhysicalFileRef>>
        = RwLock::new(HashMap::new());
    static ref FILES_BY_RELATIVE_PATH
        : RwLock<HashMap<String, Vec<PhysicalFileRef>>>
        = RwLock::new(HashMap::new());
}

/// Called by the scanner when it first finds a file. Will return its file ID
/// if the file is already in our database, or `None` if it must be deeply
/// scanned.
///
/// If we think the file is already in our database, we will add the given
/// absolute path to the list for that file.
pub fn saw_file(size: u64, _mtime: u64,
                relative_path: &str, absolute_path: &Path)
    -> Option<FileID> {
    // Check by relative path.
    let fbrp = FILES_BY_RELATIVE_PATH.read().unwrap();
    match fbrp.get(relative_path) {
        Some(x) => {
            for el in x.iter() {
                let matched = el.read().unwrap().size == size;
                if matched {
                    let mut el = el.write().unwrap();
                    el.absolute_paths.push(absolute_path.to_owned());
                    return Some(el.id)
                }
            }
        },
        None => (),
    }
    None
}

/// Called by the scanner when it has done a deep scan of a file. If the file
/// is already in the database (which can happen), checks that the given info
/// matches what we already have, and throws an error if it doesn't.
pub fn scanned_file(id: &FileID, size: u64, _mtime: u64, duration: u32,
                    relative_path: &str, absolute_path: &Path,
                    raw_meta: BTreeMap<String,String>)
    -> anyhow::Result<()> {
    use std::collections::hash_map::Entry;
    // Use writer locks because we're *fairly* sure we're gonna have to write
    // something...
    let record = {
        let mut physical_files = PHYSICAL_FILES.write().unwrap();
        match physical_files.entry(*id) {
            Entry::Occupied(ent) => {
                {
                    let mut record = ent.get().write().unwrap();
                    // this should never be a problem
                    assert_eq!(id, &record.id);
                    if size != record.size {
                        return Err(anyhow!("Same physical file, different \
                                            size? (sizes are {} and {})",
                                           size, record.size))
                    }
                    if duration != record.duration {
                        return Err(anyhow!("Same physical file, different \
                                            duration? (durations are {} and \
                                            {})", duration, record.duration))
                    }
                    if raw_meta != record.raw_meta {
                        return Err(anyhow!("Same physical file, different \
                                            physical metadata?"))
                    }
                    match record.relative_paths.iter()
                        .find(|x| *x == relative_path) {
                            // TODO: db update
                            None => record.relative_paths
                                .push(relative_path.to_owned()),
                            Some(_) => (),
                    }
                    record.absolute_paths.push(absolute_path.to_owned());
                }
                ent.get().clone()
            },
            Entry::Vacant(ent) => {
                let record = PhysicalFileRef::new(PhysicalFile {
                    id: *id, size, raw_meta, duration,
                    relative_paths: vec![relative_path.to_owned()],
                    absolute_paths: vec![absolute_path.to_owned()],
                });
                ent.insert(record.clone());
                record
            },
        }
    };
    let mut files_by_relative_path = FILES_BY_RELATIVE_PATH.write().unwrap();
    match files_by_relative_path.entry(relative_path.to_owned()) {
        Entry::Occupied(mut ent) => {
            match ent.get().iter().find(|x| &x.read().unwrap().id == id) {
                Some(_) => (),
                // TODO: assert that there are none with the same size but a
                // different ID
                None => ent.get_mut().push(record.clone()),
            }
        },
        Entry::Vacant(ent) => {
            ent.insert(vec![record.clone()]);
        },
    }
    let record = record.read().unwrap();
    let similarity_rec = SimilarityRec::new(absolute_path.file_name()
                                            .map(OsStr::to_string_lossy)
                                            .map(Cow::into_owned)
                                            .unwrap(),
                                            duration,
                                            &record.raw_meta);
    logical::incorporate_physical(id, &record.raw_meta, similarity_rec);
    Ok(())
}

/// Tries to open this `PhysicalFile` for decoding. Errors will be logged.
pub fn open_stream(id: &FileID) -> Option<ffmpeg::AVFormat> {
    let files = PHYSICAL_FILES.read().unwrap();
    let file = files.get(id)?.read().unwrap();
    for path in file.absolute_paths.iter() {
        // eprintln!("{:?}?", path);
        match ffmpeg::AVFormat::open_input(&path) {
            Ok(x) => return Some(x),
            Err(x) => {
                eprintln!("Error opening {:?}: {:?}", path, x);
                continue
            }
        }
    }
    // eprintln!("no...");
    None
}
