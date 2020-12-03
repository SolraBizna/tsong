//! This module handles *physical files*.
//!
//! This corresponds to the `physical_files` and `last_seens` tables of the
//! backing database.

use crate::*;
use lazy_static::lazy_static;

use std::{
    collections::{BTreeMap, HashMap},
    path::{Path, PathBuf},
    sync::{Arc, RwLock},
};
use anyhow::anyhow;

/// A *physical file* is a file on the disk. It contains (from our perspective)
/// exactly one *logical song*. Different encodings, etc. of the same logical
/// song correspond to different physical files.
pub struct PhysicalFile {
    /// File's SHA-256 hash.
    id: FileID,
    /// File's size. Used to help ID a file during a scan.
    size: u64,
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
        : RwLock<HashMap<FileID, Arc<RwLock<PhysicalFile>>>>
        = RwLock::new(HashMap::new());
    static ref FILES_BY_RELATIVE_PATH
        : RwLock<HashMap<String, Vec<Arc<RwLock<PhysicalFile>>>>>
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
pub fn scanned_file(id: &FileID, size: u64, _mtime: u64, relative_path: &str,
                    absolute_path: &Path, raw_meta: BTreeMap<String,String>)
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
                let record = Arc::new(RwLock::new(PhysicalFile {
                    id: *id, size, raw_meta,
                    relative_paths: vec![relative_path.to_owned()],
                    absolute_paths: vec![absolute_path.to_owned()],
                }));
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
                None => ent.get_mut().push(record),
            }
        },
        Entry::Vacant(ent) => {
            ent.insert(vec![record]);
        },
    }
    Ok(())
}
