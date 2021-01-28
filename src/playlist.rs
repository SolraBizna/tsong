//! This module handles playlists.

use crate::*;

use std::{
    collections::HashSet,
    cmp::Ordering,
    sync::RwLockReadGuard,
};

use mlua::Lua;
use lazy_static::lazy_static;

pub type PlaylistRef = Reference<Playlist>;

#[derive(Debug,Clone)]
pub struct Column {
    tag: String,
    width: i32,
}

/// A playlist is two things:
/// - An (optional) set of rules that *automatically* determine the contents
///   of a playlist. (e.g. `album:contains "Derek" and year < 2020`)
/// - A list of zero or more songs that are *unconditionally* in a playlist.
#[derive(Debug)]
pub struct Playlist {
    // serialized in database
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
    // not serialized in database
    /// The logical song generation last time we got refreshed.
    library_generation: GenerationValue,
    /// A generation tracker for this playlist.
    self_generation: GenerationTracker,
    /// List of songs, sorted.
    songs: Vec<LogicalSongRef>,
}

const PLAYLIST_CODE_LIBRARY: &str = include_str!("playlist_lib.lua");
const PLAYLIST_CODE_STUB: &str = include_str!("playlist_stub.lua");

lazy_static! {
    static ref PLAYLISTS
        : Vec<PlaylistRef>
        = Vec::new();
}

impl Playlist {
    pub fn get_name(&self) -> &str { &self.name }
    pub fn set_name(&mut self, neu: String) { self.name = neu }
    pub fn get_rule_code(&self) -> &str { &self.rule_code }
    pub fn set_rule_code(&mut self, neu: String) {
        self.rule_code = neu;
        self.library_generation.destroy();
    }
    /// Use `PlaylistRef::maybe_refreshed` instead.
    ///
    /// Update this playlist with the latest data from the logical song
    /// database, even if no changes have been made to the logical songs.
    ///
    /// Returns `Ok(())` on success, `Err("some Lua error traceback")` on
    /// failure.
    pub fn refresh(&mut self) -> Result<(), String> {
        // TODO: request fewer libraries
        // TODO 2: don't create a state at all if there's no code to run
        let lua = Lua::new();
        let compiled_song_rule = if self.rule_code.len() == 0 {
            None
        }
        else {
            match lua.load(&PLAYLIST_CODE_LIBRARY[..]).exec() {
                Ok(x) => x,
                Err(x) => return Err(format!("{}", x)),
            };
            let mut true_code
                = String::with_capacity(self.rule_code.len()
                                        +(PLAYLIST_CODE_STUB.len()-1));
            true_code += &PLAYLIST_CODE_STUB[..PLAYLIST_CODE_STUB.len()-1];
            true_code += &self.rule_code;
            let func = match lua.load(&true_code[..]).into_function() {
                Ok(x) => x,
                Err(x) => return Err(format!("{}", x)),
            };
            Some(func)
        };
        let (list, library_generation) = logical::get_all_songs_for_read();
        self.songs.clear();
        let mut seen = HashSet::new();
        for song_id in self.manually_added_ids.iter() {
            match logical::get_song_by_song_id(*song_id) {
                None => (), // TODO: warn when a manually added song is missing
                Some(song) => {
                    seen.insert(song.clone());
                    self.songs.push(song.clone());
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
                        self.songs.push(song_ref.clone())
                    },
                    Ok(false) => (),
                    Err(x) => return Err(format!("{}", x)),
                }
            }
        }
        self.library_generation = library_generation;
        self.self_generation.bump();
        self.resort();
        Ok(())
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
        &self.songs[..]
    }
    /// Sort this playlist.
    pub fn resort(&mut self) {
        let sort_order = &self.sort_order;
        self.songs.sort_by(|a, b| {
            let a = a.read().unwrap();
            let b = b.read().unwrap();
            for (ref key, desc) in sort_order {
                let a_value = a.get_metadata().get(key).map(String::as_str).unwrap_or("");
                let b_value = b.get_metadata().get(key).map(String::as_str).unwrap_or("");
                // TODO: numeric-friendly sort
                let ordering = a_value.cmp(b_value);
                let ordering = if *desc {ordering.reverse()} else {ordering};
                if ordering != Ordering::Equal { return ordering }
            }
            a.get_id().cmp(&b.get_id())
        });
    }
}

pub fn create_new_playlist() -> PlaylistRef {
    playlist::add_playlist_from_db(String::new(), String::new(),
                                   Vec::new(), Vec::new(), Vec::new())
}

pub fn add_playlist_from_db(name: String, rule_code: String,
                            manually_added_ids: Vec<SongID>,
                            columns: Vec<Column>,
                            sort_order: Vec<(String,bool)>)
    -> PlaylistRef {
    PlaylistRef::new(
        Playlist { name, rule_code, manually_added_ids, columns, sort_order,
                   library_generation: NOT_GENERATED,
                   self_generation: GenerationTracker::new(),
                   songs: Vec::new() }
    )
}

impl PlaylistRef {
    /// Returns a read lock guard for the playlist, after trying (if necessary)
    /// to refresh the playlist.
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
}
