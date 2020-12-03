//! This module is in charge of storing the user preferences values, i.e. the
//! things that are stored in `Tsong.conf` and changed in the "Preferences"
//! window.

use anyhow::anyhow;
use mlua::{Lua, HookTriggers};
use lazy_static::lazy_static;
use crate::config;

use std::{
    fs::File,
    io::{Read, Write},
    sync::RwLock,
};

#[derive(Debug)]
pub struct Preferences {
    volume: i32,
    music_paths: Vec<String>,
}

const PREFS_FILE_NAME: &str = "Tsong.conf";

/// The lowest permitted volume level.
pub const MIN_VOLUME: i32 = 0;
/// The highest permitted volume level.
pub const MAX_VOLUME: i32 = 200;

impl Default for Preferences {
    fn default() -> Self {
        Preferences {
            volume: 100,
            music_paths: Vec::new(),
        }
    }
}

lazy_static! {
    static ref PREFERENCES: RwLock<Preferences>
        = RwLock::new(Default::default());
}

/// Call at least once, at startup. This will read in saved values for the
/// preferences.
pub fn read() -> anyhow::Result<()> {
    // TODO: request the ability to load no libraries
    // TODO: report the error with Lua::new_with(StdLib::TABLE)
    let lua = Lua::new();
    lua.set_memory_limit(1000000)
        .expect("Couldn't limit configuration file memory usage");
    lua.set_hook(HookTriggers {
        on_calls: false, on_returns: false, every_line: false,
        every_nth_instruction: Some(1000000),
    }, |_lua, _debug| {
        Err(mlua::Error::RuntimeError("Configuration file(s) took too long to \
                                       execute".to_owned()))
    }).expect("Couldn't limit configuration file execution time");
    config::for_each_config_file(PREFS_FILE_NAME, |path| ->anyhow::Result<()> {
        let mut f = File::open(path)?;
        let mut a = Vec::new();
        f.read_to_end(&mut a)?;
        drop(f);
        match lua.load(&a[..]).exec() {
            Ok(_) => Ok(()),
            Err(x) => Err(anyhow!("Error in configuration file: {}", x)),
        }
    })?;
    let mut prefs = PREFERENCES.write().unwrap();
    let globals = lua.globals();
    if let Ok(Some(volume)) = globals.get::<&str, Option<i32>>("volume") {
        prefs.volume = volume.max(MIN_VOLUME).min(MAX_VOLUME)
    }
    if let Ok(Some(music_paths)) = globals
    .get::<&str, Option<Vec<String>>>("music_paths") {
        prefs.music_paths = music_paths
    }
    Ok(())
}

fn write_lua_string(f: &mut config::Update, s: &str) -> anyhow::Result<()> {
    f.write_all(b"\"")?;
    for b in s.as_bytes().iter() {
        if *b < 0x20 || *b > 0x7E { write!(f, "\\x{:02X}", b)?; }
        else if *b == b'\\' { f.write_all(b"\\\\")?; }
        else { f.write_all(&[*b])?; }
    }
    f.write_all(b"\"")?;
    Ok(())
}

/// Call to save changes to the preferences.
pub fn write() -> anyhow::Result<()> {
    let prefs = PREFERENCES.read().unwrap();
    let mut f = config::open_for_write(PREFS_FILE_NAME)?;
    writeln!(f, "-- -*- lua -*-")?;
    writeln!(f, "volume = {}", prefs.volume)?;
    writeln!(f, "music_paths = {{")?;
    for music_path in prefs.music_paths.iter() {
        f.write_all(b"  ")?;
        write_lua_string(&mut f, music_path)?;
        f.write_all(b",\n")?;
    }
    writeln!(f, "}}")?;
    f.finish()
}

/// Returns the current setting of the volume slider, bound by `MIN_VOLUME`
/// and `MAX_VOLUME`.
pub fn get_volume() -> i32 {
    PREFERENCES.read().unwrap().volume
}

/// Alters the setting of the volume slider, clamping it within `MIN_VOLUME`
/// and `MAX_VOLUME`.
pub fn set_volume(volume: i32) {
    PREFERENCES.write().unwrap().volume
        = volume.max(MIN_VOLUME).min(MAX_VOLUME)
}

/// Returns a copy of the list of music paths.
pub fn get_music_paths() -> Vec<String> {
    PREFERENCES.read().unwrap().music_paths.clone()
}

/// Replaces the list of music paths.
pub fn set_music_paths(music_paths: Vec<String>) {
    PREFERENCES.write().unwrap().music_paths = music_paths
}

