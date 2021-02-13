//! This module handles *logical songs*.
//!
//! It corresponds to the `logical_songs` table of the database.

use crate::*;
use lazy_static::lazy_static;

use std::{
    collections::{BTreeMap, HashMap},
    ffi::OsStr,
    fmt, fmt::{Display, Debug, Formatter},
    sync::{Arc, Mutex, RwLock, RwLockReadGuard},
};

pub type LogicalSongRef = Reference<LogicalSong>;

/// A song ID is a non-zero ID, unique *within the database*, that identifies a
/// particular logical song.
#[derive(Clone,Copy,PartialEq,Eq,PartialOrd,Ord,Hash)]
pub struct SongID {
    inner: u64,
}

impl Display for SongID {
    fn fmt(&self, fmt: &mut Formatter<'_>) -> fmt::Result {
        fmt.write_fmt(format_args!("{}", self.inner))
    }
}

impl Debug for SongID {
    fn fmt(&self, fmt: &mut Formatter<'_>) -> fmt::Result {
        Display::fmt(self, fmt)
    }
}

impl SongID {
    pub fn from_inner(v: u64) -> SongID {
        SongID { inner: v }
    }
    pub fn as_inner(&self) -> u64 {
        self.inner
    }
}

/// Represents some representative metadata of a *physical file*. Used as part
/// of the "same logical song" heuristic.
#[derive(Debug,Clone)]
pub struct SimilarityRec {
    pub filename: String,
    pub title: String,
    pub album: String,
    pub artist: String,
    pub duration: u32,
}

impl SimilarityRec {
    /// Applies a similarity heuristic to two files, resulting in a "similarity
    /// score". On this scale, <= 0 is definitely not the same song, >= 100 is
    /// definitely the same song, and in between is a (made up) percentage
    /// probability.
    pub fn get_similarity_to(&self, other: &SimilarityRec) -> i32 {
        let mut ret = 0;
        if self.filename == other.filename { ret += 20 }
        if self.title.len() > 0 && self.title == other.title { ret += 40 }
        if self.album.len() > 0 && self.album == other.album { ret += 30 }
        if self.artist.len() > 0 && self.artist == other.artist { ret += 30 }
        let distance = if self.duration > other.duration {
            self.duration - other.duration
        }
        else {
            other.duration - self.duration
        };
        ret += (30 - (distance.min(100) as i32) * 10).max(-20);
        ret
    }
    /// Creates a similarity record
    pub fn new(filename: String, duration: u32,
               metadata: &BTreeMap<String, String>) -> SimilarityRec {
        SimilarityRec {
            filename, duration,
            title: metadata.get("title").map(|x| x.as_str()).unwrap_or("").to_owned(),
            artist: metadata.get("artist").map(|x| x.as_str()).unwrap_or("").to_owned(),
            album: metadata.get("album").map(|x| x.as_str()).unwrap_or("").to_owned(),
        }
    }
}

/// A *logical song* is a particular performance of a particular song. It may
/// correspond to multiple *encodings* (different formats, start/end cutoffs,
/// bitrates...), each of which could be in a different *physical file*.
pub struct LogicalSong {
    // Stored in database
    id: SongID,
    user_metadata: BTreeMap<String, String>,
    physical_files: Vec<FileID>,
    duration: u32, // (duration of last played back version)
    // Not stored in database; populated as the database is loaded
    similarity_recs: Vec<SimilarityRec>,
}

static GENERATION: GenerationTracker = GenerationTracker::new();

lazy_static! {
    // Deadlock avoidance lexical order:
    // - Declaration order in this scope
    // - Any given `LogicalSong` lock (one at a time)
    // A `PhysicalFile` read lock may be held, but not a write lock. (TODO
    // tighten)
    /// Protects calls to `incorporate_physical`, preventing a race condition.
    static ref INCORPORATION_LOCK: Mutex<()> = Mutex::new(());
    static ref LOGICAL_SONGS
        : RwLock<Vec<LogicalSongRef>>
        = RwLock::new(Vec::new());
    static ref SONGS_BY_SONG_ID
        : RwLock<HashMap<SongID,LogicalSongRef>>
        = RwLock::new(HashMap::new());
    static ref SONGS_BY_FILE_ID
        : RwLock<HashMap<FileID,LogicalSongRef>>
        = RwLock::new(HashMap::new());
    static ref SONGS_BY_P_FILENAME
        : RwLock<HashMap<String,Vec<LogicalSongRef>>>
        = RwLock::new(HashMap::new());
    static ref SONGS_BY_P_TITLE
        : RwLock<HashMap<String,Vec<LogicalSongRef>>>
        = RwLock::new(HashMap::new());
    static ref SONGS_BY_P_ARTIST
        : RwLock<HashMap<String,Vec<LogicalSongRef>>>
        = RwLock::new(HashMap::new());
    static ref SONGS_BY_P_ALBUM
        : RwLock<HashMap<String,Vec<LogicalSongRef>>>
        = RwLock::new(HashMap::new());
}

/// Returns the metadata key to use for a raw FFMPEG metadata key, or `None`
/// if the given key is "unsafe".
fn map_raw_meta(k: &str) -> Option<&str> {
    match k {
        "title" | "artist" | "album" => Some(k),
        "loop_start" | "LOOP_START" => Some("loop_start"),
        "loop_end" | "LOOP_END" => Some("loop_end"),
        "disc" => Some("disc#"),
        "track" => Some("track#"),
        _ => None,
    }
}

/// Takes some raw, FFMPEG metadata, and returns the Tsong metadata we want to
/// create from it.
fn munch_ffmpeg_metadata(in_meta: &BTreeMap<String, String>,
                         duration: u32)
-> BTreeMap<String, String> {
    let mut ret = BTreeMap::new();
    ret.insert("unchecked".to_owned(), "true".to_owned());
    for (k, v) in in_meta.iter() {
        // TODO: maintain Unicode NFD
        match map_raw_meta(k) {
            Some(k) => ret.insert(k.to_owned(), v.to_owned()),
            None => ret.insert("raw_".to_owned() + k, v.to_owned()),
        };
    }
    ret.insert("duration".to_owned(), format!("{}", duration));
    ret
}

fn add_possibilities(songs: Option<&Vec<LogicalSongRef>>,
                     possibilities: &mut Vec<(LogicalSongRef, i32)>,
                     similarity_rec: &SimilarityRec)
{
    let songs = match songs { None => return, Some(x) => x };
    for song in songs.iter() {
        if !possibilities.iter().any(|x| Arc::as_ptr(&x.0) == Arc::as_ptr(song)) {
            let song = song.clone();
            let mut best_similarity = 0;
            for other_rec in song.read().unwrap().similarity_recs.iter() {
                let similarity = similarity_rec.get_similarity_to(other_rec);
                if similarity > best_similarity {
                    best_similarity = similarity;
                }
            }
            // we DO want to add this song to the list, *even if the similarity
            // score is 0*, just so we won't have to check again if the same
            // song comes up again
            possibilities.push((song, best_similarity));
        }
    }
}

/// Called by the appropriate routines in `physical` when a physical file is
/// found. We will either match this file to a logical song already in our
/// database, or make a new (fresly-imported) song.
pub fn incorporate_physical(file_id: &FileID,
                            metadata: &BTreeMap<String, String>,
                            similarity_rec: SimilarityRec) {
    let _ = INCORPORATION_LOCK.lock().unwrap();
    // physical file already incorporated? if so, nothing to do
    if let Some(_) = SONGS_BY_FILE_ID.read().unwrap().get(file_id) {
        eprintln!("Same exact song! {:?}", metadata.get("title"));
        return
    }
    // okay, but first let's see if there are any existing songs that look like
    // they might belong to this one
    let mut possibilities = Vec::new();
    add_possibilities(SONGS_BY_P_FILENAME.read().unwrap()
                      .get(&similarity_rec.filename), &mut possibilities,
                      &similarity_rec);
    add_possibilities(SONGS_BY_P_TITLE.read().unwrap()
                      .get(&similarity_rec.title), &mut possibilities,
                      &similarity_rec);
    add_possibilities(SONGS_BY_P_ARTIST.read().unwrap()
                      .get(&similarity_rec.artist), &mut possibilities,
                      &similarity_rec);
    add_possibilities(SONGS_BY_P_ALBUM.read().unwrap()
                      .get(&similarity_rec.album), &mut possibilities,
                      &similarity_rec);
    possibilities.sort_by(|a, b| b.1.cmp(&a.1));
    // now, if there is a best possibility, and that best possibility is a
    // match... match!
    let score = if possibilities.len() > 0 { possibilities[0].1 } else { 0 };
    if score >= 100 {
        // match!
        let possibility = &possibilities[0];
        eprintln!("Existing song! score = {}, title = {:?}", possibility.1, possibility.0.read().unwrap().user_metadata.get("title"));
        let mut logical_song = possibility.0.write().unwrap();
        logical_song.physical_files.push(*file_id);
        logical_song.similarity_recs.push(similarity_rec);
        db::update_song_physical_files(logical_song.id,
                                       &logical_song.physical_files);
    }
    // TODO: soft matches
    else {
        // no match! make a new song
        let new_song_ref = LogicalSongRef::new(LogicalSong {
            id: SongID::from_inner(0),
            user_metadata: munch_ffmpeg_metadata(&metadata,
                                                 similarity_rec.duration),
            physical_files: vec![*file_id],
            duration: similarity_rec.duration,
            similarity_recs: vec![similarity_rec.clone()],
        });
        let mut new_song = new_song_ref.write().unwrap();
        let song_id = db::add_song(&new_song.user_metadata,
                                   &new_song.physical_files,
                                   new_song.duration).unwrap(); // TODO: errors
        new_song.id = song_id;
        eprintln!("New song! {:?}", new_song.user_metadata.get("title"));
        drop(new_song);
        LOGICAL_SONGS.write().unwrap().push(new_song_ref.clone());
        SONGS_BY_SONG_ID.write().unwrap().insert(song_id,new_song_ref.clone());
        SONGS_BY_FILE_ID.write().unwrap().insert(*file_id,new_song_ref.clone());
        SONGS_BY_P_FILENAME.write().unwrap().entry(similarity_rec.filename)
            .or_insert_with(Vec::new).push(new_song_ref.clone());
        SONGS_BY_P_TITLE.write().unwrap().entry(similarity_rec.title)
            .or_insert_with(Vec::new).push(new_song_ref.clone());
        SONGS_BY_P_ARTIST.write().unwrap().entry(similarity_rec.artist)
            .or_insert_with(Vec::new).push(new_song_ref.clone());
        SONGS_BY_P_ALBUM.write().unwrap().entry(similarity_rec.album)
            .or_insert_with(Vec::new).push(new_song_ref.clone());
        GENERATION.bump();
    }
}

/// Fetch a logical song by its unique ID.
pub fn get_song_by_song_id(id: SongID) -> Option<LogicalSongRef> {
    SONGS_BY_SONG_ID.read().unwrap().get(&id).map(LogicalSongRef::clone)
}

/// Get the current generation of the song database. Any updates to the songs
/// will result in a bump of the underlying `GenerationTracker`.
pub fn get_generation() -> GenerationValue {
    GENERATION.snapshot()
}

/// Get a read-locked reference to the list of all logical songs, to iterate
/// through—along with the generation number at the time of the lock.
pub fn get_all_songs_for_read()
-> (RwLockReadGuard<'static, Vec<LogicalSongRef>>, GenerationValue) {
    let lock = LOGICAL_SONGS.read().unwrap();
    let generation = GENERATION.snapshot();
    (lock, generation)
}

impl LogicalSong {
    /// Returns the persistent unique ID for this song. (This is unique within
    /// the same database, not universally.)
    pub fn get_id(&self) -> SongID { self.id }
    /// Returns the full set of metadata the user has set for this song.
    pub fn get_metadata(&self) -> &BTreeMap<String, String> {
        &self.user_metadata
    }
    /// Tries to open a `PhysicalFile` of this song for decoding. Errors will
    /// be logged.
    pub fn open_stream(&self) -> Option<ffmpeg::AVFormat> {
        for id in self.physical_files.iter() {
            if let Some(x) = physical::open_stream(id) {
                return Some(x)
            }
        }
        None
    }
    /// Returns the (estimated) duration of the song, in seconds.
    pub fn get_duration(&self) -> u32 { self.duration }
    /// Updates the duration of the song. This can happen if different physical
    /// files of the song have different estimated durations because of codec
    /// differences, and a different one is chosen to be played...
    pub fn set_duration(&mut self, nu: u32) {
        if self.duration != nu {
            db::update_song_duration(self.id, nu);
            self.user_metadata.insert("duration".to_owned(), format!("{}",nu));
            // TODO: database update (metadata)
            self.duration = nu;
        }
    }
}

impl Debug for LogicalSong {
    fn fmt(&self, fmt: &mut Formatter<'_>) -> fmt::Result {
        write!(fmt, "Song ID #{}", self.id)?;
        let mut title = self.user_metadata.get("title");
        let mut artist = self.user_metadata.get("artist");
        if title.is_none() {
            for rec in self.similarity_recs.iter() {
                if rec.title.len() > 0 {
                    title = Some(&rec.title);
                    break;
                }
            }
        }
        if artist.is_none() {
            for rec in self.similarity_recs.iter() {
                if rec.artist.len() > 0 {
                    artist = Some(&rec.artist);
                    break;
                }
            }
        }
        match (title, artist) {
            (Some(title), Some(artist)) =>
                write!(fmt, ", {}, by {}", title, artist)?,
            (None, Some(artist)) =>
                write!(fmt, ", a song by {}", artist)?,
            (Some(title), None) =>
                write!(fmt, ", {}", title)?,
            _ => (),
        }
        Ok(())
    }
}

impl LogicalSongRef {
    pub fn set_duration(&self, durr: u32) {
        if self.read().unwrap().get_duration() != durr {
            self.write().unwrap().set_duration(durr)
        }
    }
}

/// Called by the database as songs are loaded.
pub fn add_song_from_db(id: SongID, user_metadata: BTreeMap<String, String>,
                        physical_files: Vec<FileID>, duration: u32) {
    let neu_ref = LogicalSongRef::new(LogicalSong {
        similarity_recs: Vec::new(),
        id, user_metadata, physical_files, duration,
    });
    let mut neu = neu_ref.write().unwrap();
    LOGICAL_SONGS.write().unwrap().push(neu_ref.clone());
    SONGS_BY_SONG_ID.write().unwrap().insert(id, neu_ref.clone());
    let mut songs_by_file_id = SONGS_BY_FILE_ID.write().unwrap();
    let mut songs_by_p_filename = SONGS_BY_P_FILENAME.write().unwrap();
    let mut songs_by_p_title = SONGS_BY_P_TITLE.write().unwrap();
    let mut songs_by_p_artist = SONGS_BY_P_ARTIST.write().unwrap();
    let mut songs_by_p_album = SONGS_BY_P_ALBUM.write().unwrap();
    let mut similarity_recs = Vec::with_capacity(neu.physical_files.len());
    for id in neu.physical_files.iter() {
        let file_ref = match physical::get_file_by_id(id) {
            Some(x) => x,
            None => {
                eprintln!("WARNING: database referenced missing file ID ({})",
                          id);
                continue
            },
        };
        let file = file_ref.read().unwrap();
        for path in file.get_absolute_paths() {
            let filename = path.file_name().map(OsStr::to_string_lossy)
                .unwrap();
            let metadata = file.get_raw_metadata();
            let similarity_rec: SimilarityRec = SimilarityRec::new(
                filename.to_owned().into(),
                file.get_duration(),
                &metadata
            );
            songs_by_p_filename.entry(similarity_rec.filename.clone())
                .or_insert_with(Vec::new).push(neu_ref.clone());
            songs_by_p_title.entry(similarity_rec.title.clone())
                .or_insert_with(Vec::new).push(neu_ref.clone());
            songs_by_p_artist.entry(similarity_rec.artist.clone())
                .or_insert_with(Vec::new).push(neu_ref.clone());
            songs_by_p_album.entry(similarity_rec.album.clone())
                .or_insert_with(Vec::new).push(neu_ref.clone());
            similarity_recs.push(similarity_rec);
        }
        songs_by_file_id.insert(*id, neu_ref.clone());
    }
    neu.similarity_recs = similarity_recs;
    GENERATION.bump();
}
