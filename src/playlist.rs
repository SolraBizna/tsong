//! This module handles playlists.

use crate::*;

use std::{
    collections::HashSet,
};

use mlua::Lua;
use lazy_static::lazy_static;

pub type PlaylistRef = Reference<Playlist>;

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
    // not serialized in database
    /// The logical song generation last time we got refreshed.
    generation: GenerationValue,
    /// List of songs, in display order.
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
        self.generation.destroy();
    }
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
        let (list, generation) = logical::get_all_songs_for_read();
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
                // TODO: apply rules
            }
        }
        self.generation = generation;
        Ok(())
    }
}

pub fn create_new_playlist() -> PlaylistRef {
    playlist::add_playlist_from_db(String::new(), String::new(),
                                   Vec::new())
}

pub fn add_playlist_from_db(name: String, rule_code: String,
                            manually_added_ids: Vec<SongID>)
    -> PlaylistRef {
    PlaylistRef::new(
        Playlist { name, rule_code, manually_added_ids,
                   generation: NOT_GENERATED, songs: Vec::new() }
    )
}
