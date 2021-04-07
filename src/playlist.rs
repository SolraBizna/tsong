//! This module handles playlists.

use crate::*;

use std::{
    collections::{HashSet, HashMap},
    cmp::Ordering,
    fmt, fmt::{Debug,Display,Formatter},
    sync::{RwLock, RwLockReadGuard, RwLockWriteGuard},
};

use alphanumeric_sort::compare_str;
use serde::{Serialize,Deserialize};
use mlua::Lua;
use lazy_static::lazy_static;
use rand::prelude::*;

pub type PlaylistRef = Reference<Playlist>;

#[derive(Clone,Copy,Debug,PartialEq,Eq)]
pub enum Playmode {
    End, Loop, LoopOne
}

#[cfg(feature="mpris")]
impl From<mpris_player::LoopStatus> for Playmode {
    fn from(i: mpris_player::LoopStatus) -> Playmode {
        match i {
            mpris_player::LoopStatus::None => Playmode::End,
            mpris_player::LoopStatus::Track => Playmode::LoopOne,
            mpris_player::LoopStatus::Playlist => Playmode::Loop,
        }
    }
}

#[cfg(feature="mpris")]
impl From<Playmode> for mpris_player::LoopStatus {
    fn from(i: Playmode) -> mpris_player::LoopStatus {
        match i {
            Playmode::End => mpris_player::LoopStatus::None,
            Playmode::LoopOne => mpris_player::LoopStatus::Track,
            Playmode::Loop => mpris_player::LoopStatus::Playlist,
        }
    }
}

impl Playmode {
    pub fn to_db_value(&self) -> i8 {
        match self {
            Playmode::End => 0,
            Playmode::Loop => 1,
            Playmode::LoopOne => 2,
        }
    }
    pub fn from_db_value(n: i64) -> Playmode {
        match n {
            1 => Playmode::Loop,
            2 => Playmode::LoopOne,
            _ => Playmode::End, // be tolerant
        }
    }
    pub fn bump(&self) -> Playmode {
        match self {
            Playmode::End => Playmode::Loop,
            Playmode::Loop => Playmode::LoopOne,
            Playmode::LoopOne => Playmode::End
        }
    }
}

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

impl PlaylistID {
    pub fn from_inner(v: u64) -> PlaylistID {
        PlaylistID { inner: v }
    }
    pub fn as_inner(&self) -> u64 {
        self.inner
    }
}

#[derive(Debug,Clone,Serialize,Deserialize,PartialEq,Eq)]
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
    /// Playback mode (whether and how to loop).
    playmode: Playmode,
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

pub const DEFAULT_COLUMN_WIDTH: u32 = 117;

lazy_static! {
    static ref TOP_LEVEL_PLAYLISTS
        : RwLock<Vec<PlaylistRef>>
        = RwLock::new(Vec::new());
    static ref PLAYLISTS_BY_ID
        : RwLock<HashMap<PlaylistID, PlaylistRef>>
        = RwLock::new(HashMap::new());
    // note: if these ever become mutable, this slightly changes the meaning
    // of the database schema
    pub static ref DEFAULT_COLUMNS
        : Vec<Column>
        = vec![
            Column{tag:"title".to_owned(),
                   width:220},
            Column{tag:"duration".to_owned(),
                   width:50},
            Column{tag:"artist".to_owned(),
                   width:DEFAULT_COLUMN_WIDTH},
            Column{tag:"album".to_owned(),
                   width:DEFAULT_COLUMN_WIDTH}
        ];
    pub static ref DEFAULT_SORT_ORDER
        : Vec<(String,bool)>
        = vec![
            ("disc#".to_owned(), false),
            ("track#".to_owned(), false),
            ("album".to_owned(), false),
            ("title".to_owned(), false),
        ];
}

impl Playlist {
    pub fn get_id(&self) -> PlaylistID { self.id }
    pub fn get_name(&self) -> &str { &self.name }
    /// Changes the name of this playlist, and also updates the database.
    pub fn set_name(&mut self, neu: String) {
        self.name = neu;
        db::update_playlist_name(self.id, &self.name)
    }
    pub fn get_rule_code(&self) -> &str { &self.rule_code }
    /// Checks the validity of the given rule code. Returns:
    /// - `Err("...")` → the rule code is invalid and we made no change
    /// - `Ok(...)` → the rule code is valid and we made the change
    pub fn set_rule_code(&mut self, neu: String) -> Result<(), String> {
        self.refresh_with_code(Some(&neu))?;
        self.rule_code = neu;
        Ok(db::update_playlist_rule_code(self.id, &self.rule_code))
    }
    /// Checks the validity of the given rule code. Returns:
    /// - `Err("...")` → the rule code is invalid and we made no change
    /// - `Ok(...)` → the rule code is valid and we made the change
    pub fn set_rule_code_and_columns(&mut self, neu_code: String,
                                     neu_columns: Vec<Column>)
    -> Result<(), String> {
        self.refresh_with_code(Some(&neu_code))?;
        if self.columns != neu_columns {
            self.self_generation.bump();
        }
        self.rule_code = neu_code;
        self.columns = neu_columns;
        Ok(db::update_playlist_rule_code_and_columns(self.id, &self.rule_code,
                                                     &self.columns))
    }
    /// Get the list of song IDs that were *manually added* to this playlist.
    /// This list should always be sorted and free of duplicates.
    pub fn get_manual_songs(&self) -> &[SongID] {
        &self.manually_added_ids[..]
    }
    /// Change the list of manually added songs. The list must already be
    /// sorted and free of duplicates.
    pub fn set_manual_songs(&mut self, songs: Vec<SongID>) {
        if self.manually_added_ids != songs {
            self.manually_added_ids = songs;
            match self.refresh() {
                Err(x) =>
                    eprintln!("Warning: Error during manually added song \
                               refresh: {}", x),
                _ => (),
            }
            db::update_playlist_manually_added_songs
                (self.id, &self.manually_added_ids[..]);
        }
    }
    pub fn get_columns(&self) -> &[Column] { &self.columns[..] }
    pub fn resize_column(&mut self, tag: &str, width: u32) {
        for column in self.columns.iter_mut() {
            if column.tag == tag {
                column.width = width;
                db::update_playlist_columns(self.id, &self.columns);
                break
            }
        }
    }
    pub fn get_sort_order(&self) -> &[(String,bool)] { &self.sort_order[..] }
    pub fn get_children(&self) -> &[PlaylistRef] { &self.children[..] }
    pub fn get_parent(&self) -> Option<PlaylistRef> {
        self.parent_id.and_then(get_playlist_by_id)
    }
    pub fn get_playmode(&self) -> Playmode { self.playmode }
    pub fn set_playmode(&mut self, nu: Playmode) {
        self.playmode = nu;
        db::update_playlist_playmode(self.id, nu)
    }
    pub fn bump_playmode(&mut self) -> Playmode {
        let nu = self.playmode.bump();
        self.set_playmode(nu);
        nu
    }
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
        db::update_playlist_sort_order_and_disable_shuffle(self.id,
                                                         &self.sort_order[..]);
        self.resort();
    }
    /// The user wants to toggle shuffle mode. Returns whether shuffle is now
    /// enabled
    pub fn toggle_shuffle(&mut self) -> bool {
        self.shuffled = !self.shuffled;
        db::update_playlist_shuffled(self.id, self.shuffled);
        self.resort();
        self.shuffled
    }
    pub fn set_shuffle(&mut self, shuffled: bool) {
        if self.shuffled != shuffled {
            self.shuffled = shuffled;
            db::update_playlist_shuffled(self.id, self.shuffled);
            self.resort();
        }
    }
    /// Returns true if the playlist is shuffled, false if it is sorted.
    pub fn is_shuffled(&self) -> bool {
        self.shuffled
    }
    fn compile_song_rule<'a>(lua: &'a Lua, rule_code: &str)
    -> Result<Option<mlua::Function<'a>>, String> {
        if rule_code.len() == 0 {
            Ok(None)
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
            Ok(Some(func))
        }
    }
    pub fn syntax_check_rule_code(rule_code: &str) -> Result<(), String> {
        let lua = Lua::new();
        Self::compile_song_rule(&lua, rule_code)?;
        Ok(())
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
        let compiled_song_rule = Self::compile_song_rule(&lua, rule_code)?;
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
    ///
    /// Returns true if the order of the playlist's contents changed as a
    /// result of the sort, false if it remained the same.
    pub fn resort(&mut self) -> bool {
        let mut newly_sorted_songs = self.unsorted_songs.clone();
        if self.shuffled {
            let mut rng = thread_rng();
            if newly_sorted_songs.len() > 1 {
                // in place sorting hat algorithm!
                for n in 0 .. newly_sorted_songs.len() - 1 {
                    let a = n;
                    let b = rng.gen_range(n+1 .. newly_sorted_songs.len());
                    newly_sorted_songs.swap(a, b);
                }
            }
        }
        else {
            let sort_order = &self.sort_order;
            newly_sorted_songs.sort_by(|a, b| {
                let a = a.read().unwrap();
                let b = b.read().unwrap();
                for (key, desc) in sort_order {
                    let a_value = a.get_metadata().get(key).map(String::as_str)
                        .unwrap_or("");
                    let b_value = b.get_metadata().get(key).map(String::as_str)
                        .unwrap_or("");
                    let ordering = compare_str(a_value, b_value);
                    let ordering = if *desc {ordering.reverse()} else {ordering};
                    if ordering != Ordering::Equal { return ordering }
                }
                a.get_id().cmp(&b.get_id())
            });
        }
        if newly_sorted_songs != self.sorted_songs {
            self.sorted_songs = newly_sorted_songs;
            self.self_generation.bump();
            true
        }
        else {
            false
        }
    }
}

pub fn create_new_playlist() -> anyhow::Result<PlaylistRef> {
    // TODO: internationalize the default playlist name. (this is otherwise
    // going to be a really easy case to miss)
    let new_playlist_name = "New Playlist".to_owned();
    let top_level_playlists = TOP_LEVEL_PLAYLISTS.read().unwrap();
    let new_order = if top_level_playlists.len() == 0 { 0 }
    else { top_level_playlists[top_level_playlists.len()-1].read().unwrap()
           .parent_order+1 };
    drop(top_level_playlists);
    let new_id = db::create_playlist(&new_playlist_name, new_order)?;
    Ok(add_playlist_from_db(new_id, None, new_order, new_playlist_name,
                            String::new(), false, Playmode::End, Vec::new(),
                            DEFAULT_COLUMNS.clone(),
                            DEFAULT_SORT_ORDER.clone()))
}

/// Add a new playlist, loaded from the database. You will need to call
/// `rebuild_children()` when you're done calling this.
pub fn add_playlist_from_db(id: PlaylistID, parent_id: Option<PlaylistID>,
                            parent_order: u64,
                            name: String, rule_code: String,
                            shuffled: bool, playmode: Playmode,
                            manually_added_ids: Vec<SongID>,
                            columns: Vec<Column>,
                            sort_order: Vec<(String,bool)>)
    -> PlaylistRef {
    let ret = PlaylistRef::new(
        Playlist { id, parent_id, parent_order, name, rule_code,
                   manually_added_ids, columns, sort_order, shuffled, playmode,
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
            db::update_playlist_parent_order(playlist.id, n as u64);
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
        db::update_playlist_parent_id_and_order(child.id, child.parent_id,
                                                child.parent_order);
        siblings.push(child_ref.clone());
        next_order += 1;
    }
    victim.children.clear(); // be tidy
}

/// Deletes a playlist from the database. Any children will be adopted by their
/// grandparents. Handles all child-related bookkeeping; you *do not* need to
/// call `rebuild_children`!
pub fn delete_playlist(victim_ref: PlaylistRef) {
    let playlists_by_id = PLAYLISTS_BY_ID.write().unwrap();
    let mut victim = victim_ref.write().unwrap();
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
    db::delete_playlist(victim.id);
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
    /// Removes this playlist from its old place in the order, and move it to
    /// be a child of the given playlist (or top-level), before the other given
    /// playlist (or at the end).
    pub fn move_next_to(&self, parent_ref: Option<&PlaylistRef>,
                        sibling_ref: Option<&PlaylistRef>) {
        // The borrow checker did not want this function to be easy to write...
        let playlists_by_id = PLAYLISTS_BY_ID.write().unwrap();
        let mut victim = self.write().unwrap();
        let mut top_level_playlists = TOP_LEVEL_PLAYLISTS
            .write().unwrap();
        match victim.parent_id.as_ref()
            .and_then(|x| playlists_by_id.get(x)) {
                None => {
                    // Orphan or top-level playlist.
                    delete_playlist_from(&self, &mut victim,
                                         &mut top_level_playlists);
                },
                Some(parent_ref) => {
                    delete_playlist_from(&self, &mut victim,
                                         &mut parent_ref.write().unwrap()
                                         .children);
                },
        }
        victim.parent_id = parent_ref.as_ref().map(|x| x.read().unwrap().get_id());
        let mut parent = parent_ref.as_ref().map(|x| x.write().unwrap());
        let children = match &mut parent {
            Some(parent) => {
                victim.parent_id = Some(parent.id);
                &mut parent.children
            },
            None => {
                victim.parent_id = None;
                &mut top_level_playlists
            },
        };
        db::update_playlist_parent_id(victim.id, victim.parent_id);
        drop(victim);
        let store_index = sibling_ref.and_then(|x| children.iter()
                                               .position(|y| y == x))
            .unwrap_or_else(|| children.len());
        children.insert(store_index, self.clone());
        redo_parent_orders(&mut children[..]);
    }
}

impl Debug for Playlist {
    fn fmt(&self, fmt: &mut Formatter<'_>) -> fmt::Result {
        write!(fmt, "Playlist ID #{}, {:?}", self.id, self.name)
    }
}

