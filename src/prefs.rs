//! This module is in charge of storing the user preferences values, i.e. the
//! things that are stored in `Tsong.conf` and changed in the "Preferences"
//! window.

use anyhow::anyhow;
use mlua::{Lua, HookTriggers};
use lazy_static::lazy_static;
use crate::config;

use std::{
    convert::TryInto,
    fs::File,
    io::{Read, Write},
    sync::RwLock,
};

use portaudio::{
    HostApiIndex,
    PortAudio,
};

#[derive(Debug)]
pub struct Preferences {
    volume: i32,
    music_paths: Vec<String>,
    // these two must both match in order for the choice to be considered valid
    audio_api_index: Option<u32>,
    audio_api_name: Option<String>,
    // same
    audio_dev_index: Option<u32>,
    audio_dev_name: Option<String>,
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
            audio_api_index: None, audio_api_name: None,
            audio_dev_index: None, audio_dev_name: None,
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
    if let Ok(audio_api_index) = globals.get("audio_api_index") {
        prefs.audio_api_index = audio_api_index;
    }
    if let Ok(audio_api_name) = globals.get("audio_api_name") {
        prefs.audio_api_name = audio_api_name;
    }
    if let Ok(audio_dev_index) = globals.get("audio_dev_index") {
        prefs.audio_dev_index = audio_dev_index;
    }
    if let Ok(audio_dev_name) = globals.get("audio_dev_name") {
        prefs.audio_dev_name = audio_dev_name;
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
    match (prefs.audio_api_index, prefs.audio_api_name.as_ref()) {
        (Some(index), Some(name)) => {
            write!(f, "\n\
                       -- PortAudio settings\n\
                       audio_api_index = {}\n\
                       audio_api_name = ", index)?;
            write_lua_string(&mut f, name)?;
            f.write_all(b"\n")?;
        },
        _ => (),
    }
    match (prefs.audio_dev_index, prefs.audio_dev_name.as_ref()) {
        (Some(index), Some(name)) => {
            match (prefs.audio_api_index, prefs.audio_api_name.as_ref()) {
                (Some(_), Some(_)) => (),
                _ => f.write_all(b"\n-- PortAudio settings\n")?,
            }
            write!(f, "audio_dev_index = {}\n\
                       audio_dev_name = ", index)?;
            write_lua_string(&mut f, name)?;
            f.write_all(b"\n")?;
        }
        _ => (),
    }
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

/// Returns the `HostApiIndex` of the audio host API chosen by the user, or of
/// the default host API if the user hasn't made a choice or if the user's
/// choice could not be found.
pub fn get_chosen_audio_api(pa: &PortAudio) -> HostApiIndex {
    let prefs = PREFERENCES.read().unwrap();
    if let Some(audio_api_index) = prefs.audio_api_index
        .and_then(|x| x.try_into().ok()) {
            if let Some(info) = pa.host_api_info(audio_api_index) {
                if let Some(audio_api_name) = prefs.audio_api_name.as_ref() {
                    if info.name == audio_api_name { return audio_api_index
                                                     as HostApiIndex }
                }
            }
        }
    return pa.default_host_api().unwrap()
}

/// Returns the device index of the audio device chosen by the user, if the
/// user has made a choice AND the chosen host API index matches the user's
/// choice of host API. Returns `None` if the user hasn't made a choice, or if
/// the passed host API index doesn't match the user's choice, or if the user's
/// choice is "use the default device".
///
/// This is a PER-API device index, hence being `u32` and not `DeviceIndex`!
pub fn get_chosen_audio_device_for_api(pa: &PortAudio,
                                       host_api: HostApiIndex) -> Option<u32> {
    let chosen_api = get_chosen_audio_api(pa);
    if chosen_api != host_api { return None }
    let prefs = PREFERENCES.read().unwrap();
    if let Some(api_dev_index) = prefs.audio_dev_index {
        let audio_dev_index
            = pa.api_device_index_to_device_index(host_api,
                                                  api_dev_index as i32);
        if let Ok(audio_dev_index) = audio_dev_index {
            if let Ok(info) = pa.device_info(audio_dev_index) {
                if info.host_api == chosen_api {
                    if let Some(audio_dev_name)=prefs.audio_dev_name.as_ref() {
                        if info.name == audio_dev_name {
                            return Some(api_dev_index as u32)
                        }
                    }
                }
            }
        }
    }
    None
}

pub fn set_chosen_audio_api_and_device(pa: &PortAudio,
                                       api_index: HostApiIndex,
                                       api_name: &str,
                                       dev: Option<(u32,&str)>) {
    let default = pa.default_host_api().unwrap();
    let mut prefs = PREFERENCES.write().unwrap();
    if api_index == default {
        prefs.audio_api_index = None;
        prefs.audio_api_name = None;
    }
    else {
        prefs.audio_api_index = Some(api_index as u32);
        prefs.audio_api_name = Some(api_name.to_owned());
    }
    match dev {
        None => {
            prefs.audio_dev_index = None;
            prefs.audio_dev_name = None;
        },
        Some((dev_index, dev_name)) => {
            prefs.audio_dev_index = Some(dev_index);
            prefs.audio_dev_name = Some(dev_name.to_owned());
        },
    }
}
