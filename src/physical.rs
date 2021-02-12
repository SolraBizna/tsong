//! This module handles *physical files*.
//!
//! This corresponds to the `physical_files` and `last_seens` tables of the
//! backing database.

use crate::*;
use lazy_static::lazy_static;
use arrayref::array_ref;

use std::{
    borrow::Cow,
    collections::{BTreeMap, HashMap, hash_map::Entry},
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
    inner: [u8; ID_SIZE],
}
pub const ID_SIZE: usize = sha256::HASHBYTES;

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
    pub fn from_bytes(bytes: &[u8]) -> anyhow::Result<FileID> {
        if bytes.len() == ID_SIZE {
            Ok(FileID::from_hash(array_ref!(bytes, 0, ID_SIZE)))
        }
        else {
            Err(anyhow!("File ID wasn't exactly {} bytes", ID_SIZE))
        }
    }
    pub fn from_hash(bytes: &[u8; ID_SIZE]) -> FileID {
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
    pub fn as_bytes(&self) -> &[u8; ID_SIZE] {
        &self.inner
    }
}

/// A *physical file* is a file on the disk. It contains (from our perspective)
/// exactly one *logical song*. Different encodings, etc. of the same logical
/// song correspond to different physical files.
#[derive(Debug)]
pub struct PhysicalFile {
    // Serialized in database
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
    /// Raw metadata, exactly as returned by FFMPEG.
    raw_meta: BTreeMap<String,String>,
    // Not serialized in database
    /// All absolute paths under which we've seen this file since startup. Used
    /// to actually find the file when it's time to play.
    absolute_paths: Vec<PathBuf>,
}

impl PhysicalFile {
    pub fn get_absolute_paths(&self) -> &[PathBuf] {
        &self.absolute_paths[..]
    }
    pub fn get_raw_metadata(&self) -> &BTreeMap<String, String> {
        &self.raw_meta
    }
    pub fn get_duration(&self) -> u32 {
        self.duration
    }
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

/// Called by the database during initial database load.
pub fn add_file_from_db(id: FileID, size: u64, duration: u32,
                        relative_paths: Vec<String>,
                        raw_meta: BTreeMap<String, String>) {
    let mut physical_files = PHYSICAL_FILES.write().unwrap();
    let mut files_by_relative_path = FILES_BY_RELATIVE_PATH.write().unwrap();
    let neu_ref = match physical_files.entry(id) {
        Entry::Occupied(_) => {
            eprintln!("WARNING: Ignoring PhysicalFile with duplicate ID from \
                       database! (id = {})", id);
            return
        },
        Entry::Vacant(ent) => {
            let record = PhysicalFileRef::new(PhysicalFile {
                id, size, raw_meta, duration, relative_paths,
                absolute_paths: vec![],
            });
            ent.insert(record.clone());
            record
        },
    };
    for path in neu_ref.read().unwrap().relative_paths.iter() {
        match files_by_relative_path.entry(path.to_owned()) {
            Entry::Occupied(mut ent) => {
                match ent.get().iter().find(|x| x.read().unwrap().id == id) {
                    Some(_) => (),
                    // TODO: assert that there are none with the same size but
                    // a different ID
                    None => ent.get_mut().push(neu_ref.clone()),
                }
            }
            Entry::Vacant(ent) => {
                ent.insert(vec![neu_ref.clone()]);
            },
        }
    }
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
                            None => {
                                record.relative_paths
                                    .push(relative_path.to_owned());
                                db::update_file_relative_paths(
                                    &record.id,
                                    &record.relative_paths);
                            },
                            Some(_) => (),
                    }
                    record.absolute_paths.push(absolute_path.to_owned());
                }
                ent.get().clone()
            },
            Entry::Vacant(ent) => {
                let record_ref = PhysicalFileRef::new(PhysicalFile {
                    id: *id, size, raw_meta, duration,
                    relative_paths: vec![relative_path.to_owned()],
                    absolute_paths: vec![absolute_path.to_owned()],
                });
                ent.insert(record_ref.clone());
                let record = record_ref.read().unwrap();
                db::add_file(&record.id, record.size, &record.raw_meta,
                             record.duration, &record.relative_paths);
                drop(record);
                record_ref
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
        match ffmpeg::AVFormat::open_input(&path) {
            Ok(x) => return Some(x),
            Err(x) => {
                eprintln!("Error opening {:?}: {:?}", path, x);
                continue
            }
        }
    }
    None
}

pub fn get_file_by_id(id: &FileID) -> Option<PhysicalFileRef> {
    PHYSICAL_FILES.read().unwrap().get(id).cloned()
}
