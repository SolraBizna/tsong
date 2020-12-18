//! This module handles *logical songs*.
//!
//! It corresponds to the `logical_songs` table of the database.

use crate::*;
use lazy_static::lazy_static;

use std::{
    collections::{BTreeMap, HashMap},
    fmt, fmt::{Display, Debug, Formatter},
    sync::{Arc, Mutex, RwLock, atomic::{Ordering, AtomicU64}, RwLockReadGuard},
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

lazy_static! {
    static ref NEXT_SONG: AtomicU64 = 1.into();
}

impl SongID {
    fn new() -> SongID {
        // TODO: let the database do this for us
        // TODO: assert < 0x7FFFFFFFFFFFFFF0 or so?
        SongID { inner: NEXT_SONG.fetch_add(1, Ordering::Relaxed) }
    }
    pub fn from_db(v: u64) -> SongID {
        SongID { inner: v }
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
#[derive(Debug)]
pub struct LogicalSong {
    // Stored in database
    id: SongID,
    user_metadata: BTreeMap<String, String>,
    physical_files: Vec<FileID>,
    // Not stored in database; populated as the database is loaded
    similarity_recs: Vec<SimilarityRec>,
}

impl LogicalSong {
    pub fn get_id(&self) -> SongID { self.id }
    pub fn get_metadata(&self) -> &BTreeMap<String, String> {
        &self.user_metadata
    }
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

/// Returns whether the passed metadata key is "safe" to consume raw from
/// FFMPEG metadata.
fn is_safe_raw_meta(k: &str) -> bool {
    k == "title" || k == "artist" || k == "album"
}

/// Takes some raw, FFMPEG metadata, and returns the Tsong metadata we want to
/// create from it.
fn munch_ffmpeg_metadata(in_meta: &BTreeMap<String, String>)
-> BTreeMap<String, String> {
    let mut ret = BTreeMap::new();
    ret.insert("unchecked".to_owned(), "true".to_owned());
    for (k, v) in in_meta.iter() {
        if is_safe_raw_meta(k) { ret.insert(k.to_owned(), v.to_owned()); }
        else { ret.insert("raw_".to_owned() + k, v.to_owned()); }
    }
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
        todo!();
    }
    // TODO: soft match
    else {
        // no match! make a new song
        let song_id = SongID::new();
        let new_song = LogicalSongRef::new(LogicalSong {
            id: song_id,
            user_metadata: munch_ffmpeg_metadata(&metadata),
            physical_files: vec![*file_id],
            similarity_recs: vec![similarity_rec.clone()],
        });
        eprintln!("New song! {:?}", new_song.read().unwrap().user_metadata.get("title"));
        LOGICAL_SONGS.write().unwrap().push(new_song.clone());
        SONGS_BY_SONG_ID.write().unwrap().insert(song_id, new_song.clone());
        SONGS_BY_FILE_ID.write().unwrap().insert(*file_id, new_song.clone());
        SONGS_BY_P_FILENAME.write().unwrap().entry(similarity_rec.filename)
            .or_insert_with(Vec::new).push(new_song.clone());
        SONGS_BY_P_TITLE.write().unwrap().entry(similarity_rec.title)
            .or_insert_with(Vec::new).push(new_song.clone());
        SONGS_BY_P_ARTIST.write().unwrap().entry(similarity_rec.artist)
            .or_insert_with(Vec::new).push(new_song.clone());
        SONGS_BY_P_ALBUM.write().unwrap().entry(similarity_rec.album)
            .or_insert_with(Vec::new).push(new_song.clone());
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
