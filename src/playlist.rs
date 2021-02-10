//! This module handles playlists.

use crate::*;

use std::{
    collections::{HashSet, HashMap},
    cmp::Ordering,
    fmt, fmt::{Debug,Display,Formatter},
    sync::{atomic::AtomicU64, RwLock, RwLockReadGuard, RwLockWriteGuard},
};

use mlua::Lua;
use lazy_static::lazy_static;
use rand::prelude::*;

pub type PlaylistRef = Reference<Playlist>;

/// A playlist ID is a non-zero ID, unique *within the database*, that
/// identifies a particular unique playlist.
#[derive(Clone,Copy,PartialEq,Eq,PartialOrd,Ord,Hash)]
pub struct PlaylistID {
    inner: u64,
}

impl Display for PlaylistID {
    fn fmt(&self, fmt: &mut Formatter<'_>) -> fmt::Result {
        fmt.write_fmt(format_args!("{}", self.inner))
    }
}

impl Debug for PlaylistID {
    fn fmt(&self, fmt: &mut Formatter<'_>) -> fmt::Result {
        Display::fmt(self, fmt)
    }
}

lazy_static! {
    static ref NEXT_PLAYLIST: AtomicU64 = 2401.into();
}

impl PlaylistID {
    fn new() -> PlaylistID {
        // TODO: let the database do this for us
        let inner = NEXT_PLAYLIST.fetch_add(1, std::sync::atomic::Ordering::Relaxed);
        assert!(inner < 0x7FFFFFFFFFFFFFF0u64);
        PlaylistID { inner }
    }
    pub fn from_inner(v: u64) -> PlaylistID {
        PlaylistID { inner: v }
    }
    pub fn as_inner(&self) -> u64 {
        self.inner
    }
}

#[derive(Debug,Clone)]
pub struct Column {
    pub tag: String,
    pub width: u32,
}

/// A playlist is two things:
/// - An (optional) set of rules that *automatically* determine the contents
///   of a playlist. (e.g. `album:contains "Derek" and year < 2020`)
/// - A list of zero or more songs that are *unconditionally* in a playlist.
pub struct Playlist {
    // serialized in database
    /// Unique ID of playlist.
    id: PlaylistID,
    /// Parent playlist ID.
    parent_id: Option<PlaylistID>,
    /// Order within parent playlist or global list.
    parent_order: u64,
    /// Name of playlist, set by user.
    name: String,
    /// The rules for automatically adding song to this playlist. If empty, no
    /// songs will be automatically added.
    rule_code: String,
    /// List of songs that have been manually added to this playlist.
    manually_added_ids: Vec<SongID>,
    /// List of metadata tags that are present as columns in this playlist's
    /// interface.
    columns: Vec<Column>,
    /// Metadata tags for sorting this playlist, in descending order of
    /// priority. `true` = descending, `false` = ascending.
    sort_order: Vec<(String,bool)>,
    /// True if shuffled, false if sorted.
    shuffled: bool,
    // not serialized in database
    /// The logical song generation last time we got refreshed.
    library_generation: GenerationValue,
    /// A generation tracker for the *songs* in this playlist.
    self_generation: GenerationTracker,
    /// List of songs, unsorted.
    unsorted_songs: Vec<LogicalSongRef>,
    /// List of songs, sorted/shuffled
    sorted_songs: Vec<LogicalSongRef>,
    /// References to child playlists
    children: Vec<PlaylistRef>,
}

const PLAYLIST_CODE_LIBRARY: &str = include_str!("lua/playlist_lib.lua");
const PLAYLIST_CODE_STUB: &str = include_str!("lua/playlist_stub.lua");

lazy_static! {
    static ref TOP_LEVEL_PLAYLISTS
        : RwLock<Vec<PlaylistRef>>
        = RwLock::new(Vec::new());
    static ref PLAYLISTS_BY_ID
        : RwLock<HashMap<PlaylistID, PlaylistRef>>
        = RwLock::new(HashMap::new());
    pub static ref DEFAULT_COLUMNS
        : Vec<Column>
        = vec![
            Column{tag:"title".to_owned(),
                   width:220},
            Column{tag:"duration".to_owned(),
                   width:50},
            Column{tag:"artist".to_owned(),
                   width:117},
            Column{tag:"album".to_owned(),
                   width:117}
        ];
}

impl Playlist {
    pub fn get_id(&self) -> PlaylistID { self.id }
    pub fn get_name(&self) -> &str { &self.name }
    pub fn set_name(&mut self, neu: String) {
        self.name = neu;
        // TODO: database update
    }
    pub fn get_rule_code(&self) -> &str { &self.rule_code }
    pub fn set_rule_code(&mut self, neu: String) -> Result<(), String> {
        self.refresh_with_code(Some(&neu))?;
        self.rule_code = neu;
        // TODO: database update
        Ok(())
    }
    pub fn get_columns(&self) -> &[Column] { &self.columns[..] }
    pub fn resize_column(&mut self, tag: &str, width: u32) {
        for column in self.columns.iter_mut() {
            if column.tag == tag {
                column.width = width;
                // TODO: database update
            }
        }
    }
    pub fn get_sort_order(&self) -> &[(String,bool)] { &self.sort_order[..] }
    pub fn get_children(&self) -> &[PlaylistRef] { &self.children[..] }
    /// The user clicked on a column heading.
    /// - Shuffle is disabled, if enabled.
    /// - If this is not in the order at all, add it to the front in ascending
    ///   order.
    /// - If this is already in the order, but not at the front, move it to the
    ///   front.
    /// - If this is already the front of the order, AND shuffle was already
    ///   disabled, toggle between ascending and descending order.
    pub fn touched_heading(&mut self, tag: &str) {
        let orig_pos = self.sort_order.iter().position(|x| x.0 == tag);
        match orig_pos {
            None =>
                self.sort_order.insert(0, (tag.to_owned(),false)),
            Some(0) => {
                if !self.shuffled {
                    self.sort_order[0].1 = !self.sort_order[0].1;
                }
            }
            Some(x) => {
                let auf = self.sort_order.remove(x);
                self.sort_order.insert(0, auf);
            },
        }
        self.shuffled = false;
        // TODO: database update
        self.resort();
    }
    /// The user wants to toggle shuffle mode.
    pub fn toggle_shuffle(&mut self) {
        self.shuffled = !self.shuffled;
        // TODO: database update
        self.resort();
    }
    /// Returns true if the playlist is shuffled, false if it is sorted.
    pub fn is_shuffled(&self) -> bool {
        self.shuffled
    }
    fn refresh_with_code(&mut self, rule_code: Option<&str>)
    -> Result<(), String> {
        let rule_code = match rule_code {
            None => &self.rule_code,
            Some(x) => x,
        };
        // TODO: request fewer libraries
        // TODO 2: don't create a state at all if there's no code to run
        let lua = Lua::new();
        let compiled_song_rule = if rule_code.len() == 0 {
            None
        }
        else {
            match lua.load(&PLAYLIST_CODE_LIBRARY[..]).exec() {
                Ok(x) => x,
                Err(x) => return Err(format!("{}", x)),
            };
            let mut true_code
                = String::with_capacity(rule_code.len()
                                        +(PLAYLIST_CODE_STUB.len()-1));
            true_code += &PLAYLIST_CODE_STUB[..PLAYLIST_CODE_STUB.len()-1];
            true_code += &rule_code;
            let func = match lua.load(&true_code[..]).into_function() {
                Ok(x) => x,
                Err(x) => return Err(format!("{}", x)),
            };
            Some(func)
        };
        let (list, library_generation) = logical::get_all_songs_for_read();
        let mut new_songs = Vec::new();
        let mut seen = HashSet::new();
        for song_id in self.manually_added_ids.iter() {
            match logical::get_song_by_song_id(*song_id) {
                None => (), // TODO: warn when a manually added song is missing
                Some(song) => {
                    seen.insert(song.clone());
                    new_songs.push(song.clone());
                },
            }
        }
        if let Some(func) = compiled_song_rule {
            for song_ref in list.iter() {
                if seen.contains(&song_ref) { continue }
                // not to be confused with a metatable
                let metadata_table = lua.create_table_from(song_ref.read().unwrap().get_metadata().iter().map(|(a,b)| (a.as_str(), b.as_str())));
                match func.call::<_, bool>(metadata_table) {
                    Ok(true) => {
                        new_songs.push(song_ref.clone())
                    },
                    Ok(false) => (),
                    Err(x) => return Err(format!("{}", x)),
                }
            }
        }
        if self.unsorted_songs != new_songs {
            self.unsorted_songs = new_songs;
            self.resort();
        }
        self.library_generation = library_generation;
        Ok(())
    }
    /// Use `PlaylistRef::maybe_refreshed` instead.
    ///
    /// Update this playlist with the latest data from the logical song
    /// database, even if no changes have been made to the logical songs.
    ///
    /// Returns `Ok(())` on success, `Err("some Lua error traceback")` on
    /// failure.
    pub fn refresh(&mut self) -> Result<(), String> {
        self.refresh_with_code(None)
    }
    /// Returns the logical song playlist generation value for which this
    /// playlist's contents are up to date. This is NOT a generation value for
    /// this playlist!
    pub fn get_library_generation(&self) -> GenerationValue {
        self.library_generation
    }
    /// Returns a generation value that updates at least as often as this
    /// playlist. (Currently, it can also get bumped at other times.)
    pub fn get_playlist_generation(&self) -> GenerationValue {
        self.self_generation.snapshot()
    }
    /// Get the list of songs, as of the last playlist update.
    pub fn get_songs(&self) -> &[LogicalSongRef] {
        &self.sorted_songs[..]
    }
    /// Sort (or shuffle) this playlist.
    pub fn resort(&mut self) {
        self.sorted_songs = self.unsorted_songs.clone();
        if self.shuffled {
            // in place sorting hat algorithm!
            let mut rng = thread_rng();
            if self.sorted_songs.len() <= 1 { return }
            for n in 0 .. self.sorted_songs.len() - 1 {
                let a = n;
                let b = rng.gen_range(n+1 .. self.sorted_songs.len());
                self.sorted_songs.swap(a, b);
            }
        }
        else {
            let sort_order = &self.sort_order;
            self.sorted_songs.sort_by(|a, b| {
                let a = a.read().unwrap();
                let b = b.read().unwrap();
                for (key, desc) in sort_order {
                    let a_value = a.get_metadata().get(key).map(String::as_str)
                        .unwrap_or("");
                    let b_value = b.get_metadata().get(key).map(String::as_str)
                        .unwrap_or("");
                    // TODO: numeric-friendly, and otherwise internationalized,
                    // sort
                    let ordering = a_value.cmp(b_value);
                    let ordering = if *desc {ordering.reverse()} else {ordering};
                    if ordering != Ordering::Equal { return ordering }
                }
                a.get_id().cmp(&b.get_id())
            });
        }
        self.self_generation.bump();
    }
}

pub fn create_new_playlist() -> PlaylistRef {
    // TODO: internationalize the default playlist name. (this is otherwise
    // going to be a really easy case to miss)
    playlist::add_playlist_from_db(PlaylistID::new(), None, u64::MAX,
                                   "New Playlist".to_owned(),
                                   String::new(),
                                   false,
                                   Vec::new(),
                                   DEFAULT_COLUMNS.clone(),
                                   Vec::new())
}

/// Add a new playlist, loaded from the database. You will need to call
/// `rebuild_children()` when you're done calling this.
pub fn add_playlist_from_db(id: PlaylistID, parent_id: Option<PlaylistID>,
                            parent_order: u64,
                            name: String, rule_code: String,
                            shuffled: bool,
                            manually_added_ids: Vec<SongID>,
                            columns: Vec<Column>,
                            sort_order: Vec<(String,bool)>)
    -> PlaylistRef {
    let ret = PlaylistRef::new(
        Playlist { id, parent_id, parent_order, name, rule_code,
                   manually_added_ids, columns, sort_order, shuffled,
                   library_generation: NOT_GENERATED,
                   self_generation: GenerationTracker::new(),
                   unsorted_songs: Vec::new(), sorted_songs: Vec::new(),
                   children: Vec::new() }
    );
    if parent_id.is_none() {
        TOP_LEVEL_PLAYLISTS.write().unwrap().push(ret.clone());
    }
    // we would use expect_none here but it's still an unstable feature...
    PLAYLISTS_BY_ID.write().unwrap().insert(id, ret.clone());
    ret
}

fn compare_playlists(a: &PlaylistRef, b: &PlaylistRef) -> Ordering {
    let a = a.read().unwrap();
    let b = b.read().unwrap();
    match a.parent_order.cmp(&b.parent_order) {
        Ordering::Equal => a.id.cmp(&b.id),
        x => x,
    }
}

/// Rewrites the `parent_order` fields of each playlist in the array, to start
/// at 0 and ascend without gaps. Just minor housekeeping.
fn redo_parent_orders(group: &mut[PlaylistRef]) {
    // TODO: modify to allow small gaps, to reduce database thrash
    for n in 0..group.len() {
        let mut playlist = group[n].write().unwrap();
        if playlist.parent_order != n as u64 {
            // TODO: database update
            playlist.parent_order = n as u64;
        }
    }
}

/// Call after you're finished adding the playlists from the database.
pub fn rebuild_children() {
    let mut top_level_playlists = TOP_LEVEL_PLAYLISTS.write().unwrap();
    let playlists_by_id = PLAYLISTS_BY_ID.read().unwrap();
    for playlist_ref in playlists_by_id.values() {
        let mut playlist = playlist_ref.write().unwrap();
        playlist.children.clear();
    }
    for child_ref in playlists_by_id.values() {
        let mut child = child_ref.write().unwrap();
        let parent_id = match child.parent_id {
            Some(x) => x,
            None => continue,
        };
        if parent_id == child.id {
            eprintln!("Warning: Playlist {:?} wanted to be its own parent!",
                      child_ref);
            child.parent_id = None;
            // TODO: database update?
            continue;
        }
        let parent = match playlists_by_id.get(&parent_id) {
            Some(x) => x,
            None => {
                eprintln!("Warning: Playlist {:?} wanted a parent that \
                           didn't exist!", child_ref);
                child.parent_id = None;
                // TODO: database update?
                continue;
            },
        };
        parent.write().unwrap().children.push(child_ref.clone());
    }
    top_level_playlists.sort_by(compare_playlists);
    redo_parent_orders(&mut top_level_playlists[..]);
    for playlist_ref in playlists_by_id.values() {
        let mut playlist = playlist_ref.write().unwrap();
        playlist.children.sort_by(compare_playlists);
        redo_parent_orders(&mut playlist.children[..]);
    }
}

pub fn get_top_level_playlists() -> RwLockReadGuard<'static, Vec<PlaylistRef>>{
    TOP_LEVEL_PLAYLISTS.read().unwrap()
}

pub fn get_playlist_by_id(id: PlaylistID) -> Option<PlaylistRef> {
    PLAYLISTS_BY_ID.read().unwrap().get(&id).cloned()
}

fn delete_playlist_from(victim_ref: &PlaylistRef,
                        victim: &mut RwLockWriteGuard<Playlist>,
                        siblings: &mut Vec<PlaylistRef>){
    siblings.retain(|x| x != victim_ref);
    let mut next_order = if siblings.len() == 0 { 0 }
    else { siblings[siblings.len()-1].read().unwrap().parent_order + 1 };
    for child_ref in victim.children.iter() {
        if child_ref == victim_ref { continue } // be defensive
        let mut child = child_ref.write().unwrap();
        child.parent_id = victim.parent_id;
        child.parent_order = next_order;
        // TODO: database update
        siblings.push(child_ref.clone());
        next_order += 1;
    }
    victim.children.clear(); // be tidy
}

/// Deletes a playlist from the database. Any children will be adopted by their
/// grandparents. Handles all child-related bookkeeping; you *do not* need to
/// call `rebuild_children`!
pub fn delete_playlist(victim_ref: PlaylistRef) {
    let mut victim = victim_ref.write().unwrap();
    let playlists_by_id = PLAYLISTS_BY_ID.write().unwrap();
    match victim.parent_id.as_ref().and_then(|x| playlists_by_id.get(x)) {
        None => {
            // Orphan or top-level playlist.
            let mut top_level_playlists = TOP_LEVEL_PLAYLISTS.write().unwrap();
            delete_playlist_from(&victim_ref, &mut victim,
                                 &mut top_level_playlists);
        },
        Some(parent) => {
            delete_playlist_from(&victim_ref, &mut victim,
                                 &mut parent.write().unwrap().children);
        },
    }
    victim.parent_id = None;
    // TODO: database update
}

impl PlaylistRef {
    /// Returns a read lock guard for the playlist, after trying (if necessary)
    /// to refresh (and possibly resort) the playlist.
    pub fn maybe_refreshed(&self) -> RwLockReadGuard<Playlist> {
        loop {
            let maybe = self.read().unwrap();
            if maybe.library_generation == logical::get_generation() {
                return maybe
            }
            drop(maybe);
            let mut maybe = match self.try_write() {
                Ok(x) => x,
                _ => {
                    // Trying to acquire a write lock failed. That means
                    // somebody else already tried to acquire it.
                    //
                    // That means that trying to acquire a read lock again will
                    // block. That's fine.
                    continue
                }
            };
            match maybe.refresh() {
                // It's refreshed. We're good. Let the next iteration return
                // a read guard.
                Ok(_) => continue,
                // Refreshing failed. Let the GUI handle it, and just return
                // the current state of the self.
                Err(_) => {
                    drop(maybe);
                    return self.read().unwrap()
                }
            }
        }
    }
    /// As `maybe_refreshed`, but will return `None` if the lock could not
    /// immediately be taken.
    pub fn sheepishly_maybe_refreshed(&self)
    -> Option<RwLockReadGuard<Playlist>> {
        loop {
            let maybe = match self.try_read() {
                Ok(x) => x,
                _ => return None,
            };
            if maybe.library_generation == logical::get_generation() {
                return Some(maybe)
            }
            drop(maybe);
            let mut maybe = match self.try_write() {
                Ok(x) => x,
                _ => return None,
            };
            match maybe.refresh() {
                // It's refreshed. We're good. Let the next iteration return
                // a read guard.
                Ok(_) => continue,
                // Refreshing failed. Let the GUI handle it, and just return
                // the current state of the self.
                Err(_) => {
                    drop(maybe);
                    return self.try_read().ok()
                }
            }
        }
    }
}

impl Debug for Playlist {
    fn fmt(&self, fmt: &mut Formatter<'_>) -> fmt::Result {
        write!(fmt, "Playlist ID #{}, {:?}", self.id, self.name)
    }
}

