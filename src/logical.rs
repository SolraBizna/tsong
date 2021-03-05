//! This module handles *logical songs*.
//!
//! It corresponds to the `logical_songs` table of the database.

use crate::*;

use anyhow::anyhow;
use lazy_static::lazy_static;
use mlua::{Lua, Function, Table};

use std::{
    borrow::Cow,
    cell::RefCell,
    collections::{BTreeMap, HashMap},
    convert::TryInto,
    ffi::OsStr,
    fmt, fmt::{Display, Debug, Formatter},
    io::{Read, Write},
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
pub fn incorporate_physical(file_ref: PhysicalFileRef) {
    let file = file_ref.read().unwrap();
    let duration = file.get_duration();
    let absolute_path = file.get_absolute_paths().last().unwrap();
    let metadata = file.get_raw_metadata();
    let similarity_rec = SimilarityRec::new(absolute_path.file_name()
                                            .map(OsStr::to_string_lossy)
                                            .map(Cow::into_owned)
                                            .unwrap(),
                                            duration,
                                            &metadata);
    let _ = INCORPORATION_LOCK.lock().unwrap();
    // physical file already incorporated? if so, nothing to do
    if let Some(_) = SONGS_BY_FILE_ID.read().unwrap().get(file.get_id()) {
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
        logical_song.physical_files.push(*file.get_id());
        logical_song.similarity_recs.push(similarity_rec);
        db::update_song_physical_files(logical_song.id,
                                       &logical_song.physical_files);
    }
    // TODO: soft matches
    else {
        // no match! make a new song
        let new_song_ref = LogicalSongRef::new(LogicalSong {
            id: SongID::from_inner(0),
            user_metadata: BTreeMap::new(),
            physical_files: vec![*file.get_id()],
            duration: similarity_rec.duration,
            similarity_recs: vec![similarity_rec.clone()],
        });
        let mut new_song = new_song_ref.write().unwrap();
        if let Err(x) = new_song.import_metadata(&file) {
            // TODO: error reporting, better
            eprintln!("Error while importing metadata for song on initial \
                       scan:\n{}\nFalling back to simple import.\n",
                      x);
            let mut new_metadata = BTreeMap::new();
            for (k, v) in file.get_raw_metadata().iter() {
                match k.as_str() {
                    "artist" | "album" | "title"
                        => new_metadata.insert(k.clone(), v.clone()),
                    x => new_metadata.insert("raw_".to_owned() + x, v.clone()),
                };
            }
            new_song.user_metadata = new_metadata;
        }
        let song_id = db::add_song(&new_song.user_metadata,
                                   &new_song.physical_files,
                                   new_song.duration).unwrap(); // TODO: errors
        new_song.id = song_id;
        eprintln!("New song! {:?}", new_song.user_metadata.get("title"));
        drop(new_song);
        LOGICAL_SONGS.write().unwrap().push(new_song_ref.clone());
        SONGS_BY_SONG_ID.write().unwrap().insert(song_id,new_song_ref.clone());
        SONGS_BY_FILE_ID.write().unwrap().insert(*file.get_id(),new_song_ref.clone());
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

lazy_static! {
    static ref SCRIPT_GENERATION: GenerationTracker = GenerationTracker::new();
    static ref IMPORT_SCRIPT_LOCK: Mutex<()> = Mutex::new(());
}

thread_local! {
    static TLS: RefCell<(GenerationValue, Option<Lua>)>
        = RefCell::new((Default::default(), None));
}

const IMPORT_LIB: &[u8] = include_bytes!("lua/importlib.lua");
const DEFAULT_IMPORT_SCRIPT: &[u8] = include_bytes!("lua/import.lua.example");
const IMPORT_FUNC_KEY: &[u8] = b"Tsong Metadata Import Script";

fn try_get_import_script() -> anyhow::Result<Option<Vec<u8>>> {
    if let Some(mut f) = config::open_best_for_read("import.lua")? {
        let mut ret = Vec::new();
        f.read_to_end(&mut ret)?;
        Ok(Some(ret))
    }
    else {
        Ok(None)
    }
}

/// `mlua::Error` is not `Send`, so we can't put it through `anyhow` without
/// a little bit of glue.
trait MakeLuaErrorSyncSafe {
    type Wat;
    fn anyhowify(self) -> anyhow::Result<Self::Wat>;
}

impl<T> MakeLuaErrorSyncSafe for mlua::Result<T> {
    type Wat = T;
    fn anyhowify(self) -> anyhow::Result<Self::Wat> {
        match self {
            Ok(x) => Ok(x),
            Err(x) => Err(anyhow!("{}", x)),
        }
    }
}

fn load_import_script(lua: &Lua) -> anyhow::Result<Function> {
    let script_func = match try_get_import_script() {
        Ok(Some(x)) => {
            lua.load(&x[..])
                .set_name("import.lua").unwrap()
                .into_function().anyhowify()
                .map(|x| Some(x))
        },
        Ok(None) => Ok(None),
        Err(x) => Err(x)
    };
    let script_func = match script_func {
        Ok(x) => x,
        Err(x) => {
            eprintln!("Error loading user-provided \
                       \"import.lua\". Using the built-in \
                       script.\n{}\n", x);
            None
        },
    };
    let script_func = match script_func {
        Some(x) => x,
        None => {
            lua.load(DEFAULT_IMPORT_SCRIPT)
                .set_name("import.lua").unwrap()
                .into_function().anyhowify()?
        }
    };
    Ok(script_func)
}

impl LogicalSong {
    pub fn import_metadata(&mut self, file: &PhysicalFile)
    -> anyhow::Result<()> {
        let res = TLS.with(|cell| -> anyhow::Result<()> {
            let mut cellref = cell.borrow_mut();
            let (ref mut last_load_generation, ref mut lua_state) = *cellref;
            if lua_state.is_none()
            || !SCRIPT_GENERATION.has_not_changed_since(last_load_generation) {
                *lua_state = None;
                *last_load_generation = SCRIPT_GENERATION.snapshot();
                let lua = Lua::new();
                lua.load(IMPORT_LIB).set_name("importlib.lua").unwrap()
                    .exec().anyhowify()?;
                let script_func = load_import_script(&lua)?;
                lua.set_named_registry_value(IMPORT_FUNC_KEY, script_func)
                    .unwrap();
                *lua_state = Some(lua);
            }
            let lua = lua_state.as_ref().unwrap();
            // Script is in place. Go, go, go!
            // Set up the globals...
            let globals = lua.globals();
            let inmeta = lua.create_table_from(file.get_raw_metadata().iter().map(|(a,b)| (a.as_str(), b.as_str()))).anyhowify()?;
            globals.raw_set("inmeta", inmeta).anyhowify()?;
            let outmeta = lua.create_table_from(self.user_metadata.iter().map(|(a,b)| (a.as_str(), b.as_str()))).anyhowify()?;
            globals.raw_set("outmeta", outmeta).anyhowify()?;
            globals.raw_set("filename", file.get_absolute_paths()[0].file_name().unwrap().to_string_lossy().into_owned()).anyhowify()?;
            globals.raw_set("path", file.get_absolute_paths()[0].to_string_lossy().into_owned()).anyhowify()?;
            let filenames = lua.create_table_from(file.get_absolute_paths().iter().enumerate().map(|(i, x)| (i+1, x.file_name().unwrap().to_string_lossy().into_owned()))).anyhowify()?;
            globals.raw_set("filenames", filenames).anyhowify()?;
            let paths = lua.create_table_from(file.get_absolute_paths().iter().enumerate().map(|(i, x)| (i+1, x.to_string_lossy().into_owned()))).anyhowify()?;
            globals.raw_set("paths", paths).anyhowify()?;
            globals.raw_set("file_id", file.get_id().to_string()).anyhowify()?;
            let song_id: Option<i64> = if self.id.inner == 0 { None }
            else { Some(self.id.inner.try_into().unwrap()) };
            globals.raw_set("song_id", song_id).anyhowify()?;
            let func: Function
                = lua.named_registry_value(IMPORT_FUNC_KEY).unwrap();
            // TODO: handle errors...
            let _: () = func.call(()).anyhowify()?;
            let mut new_metadata = BTreeMap::new();
            let outmeta: Table = globals.raw_get("outmeta").anyhowify()?;
            for res in outmeta.pairs() {
                let (k, v) = res.anyhowify()?;
                new_metadata.insert(k, v);
            }
            new_metadata.insert("duration".to_owned(),
                                format!("{}", file.get_duration()));
            self.user_metadata = new_metadata;
            Ok(())
        });
        match res {
            Ok(_) => Ok(()),
            Err(x) => Err(anyhow!("{}", x)),
        }
    }
}

/// Checks to see if `import.lua.example` needs to be created or updated. Call
/// once, on startup. Maybe even spawn it into a thread!
pub fn maybe_write_example_import_script() -> Option<()> {
    if let Ok(Some(mut f)) = config::open_best_for_read("import.lua.example") {
        let mut buf = Vec::new();
        if let Ok(_) = f.read_to_end(&mut buf) {
            if buf == DEFAULT_IMPORT_SCRIPT {
                return None
            }
        }
    }
    let mut f = config::open_for_write("import.lua.example").ok()?;
    f.write_all(DEFAULT_IMPORT_SCRIPT).ok()?;
    f.finish().ok()?;
    None
}
