//! This module locates and handles configuration files.
//!
//! TODO: explain configuration path logic, including backups and "neuen"

/// The suffix to apply to the *current* version of a file, when backing it up
/// before replacement.
#[cfg(target_os = "windows")]
pub const BACKUP_SUFFIX: &str = ".bak";
/// The suffix to apply to the *current* version of a file, when backing it up
/// before replacement.
#[cfg(not(target_os = "windows"))]
pub const BACKUP_SUFFIX: &str = "~";

/// The suffix to apply to the *new* version of a file, while still writing it.
#[cfg(target_os = "windows")]
pub const NEW_SUFFIX: &str = ".neu";
/// The suffix to apply to the *new* version of a file, while still writing it.
#[cfg(not(target_os = "windows"))]
pub const NEW_SUFFIX: &str = "^";

#[cfg(target_family = "unix")]
use std::os::unix::ffi::OsStrExt;
#[cfg(target_family = "unix")]
use std::ffi::OsStr;

use std::{
    env::var_os,
    fs,
    fs::File,
    io::Write,
    ops::{Deref, DerefMut},
    path::{Path,PathBuf},
};
use lazy_static::lazy_static;
use anyhow::Context;

lazy_static! {
    /// The list of search paths for configuration files, in order from most
    /// general to most specific. Configuration files found in later
    /// directories should override files found in earlier ones. When writing,
    /// writing should be attempted starting at the *end* of the list.
    static ref CONFIG_PATHS: Vec<PathBuf> = get_search_paths();
}

/// Internal function, used to populate `CONFIG_PATHS`.
fn get_search_paths() -> Vec<PathBuf> {
    let mut ret: Vec<PathBuf> = Vec::new();
    if let Some(config_path) = var_os("TSONG_CONFIG_HOME") {
        // `TSONG_CONFIG_HOME` overrides all other configuration paths. This
        // allows "portable Tsong".
        ret.push(config_path.into());
    }
    else if cfg!(target_os = "windows") {
        // Preferred: %AppData%\
        if let Some(app_data) = var_os("APPDATA") {
            ret.push(app_data.into());
        }
        // Acceptable: %UserProfile%\AppData\Roaming\
        // Debatable: %HomePath%\AppData\Roaming\
        // Defensible: %Home%\AppData\Roaming\
        else if let Some(user_profile) = var_os("USERPROFILE")
            .or_else(|| var_os("HOMEPATH"))
            .or_else(|| var_os("HOME")) {
                let mut buf: PathBuf = user_profile.into();
                buf.push("AppData");
                buf.push("Roaming");
                ret.push(buf);
        }
        // Getting desperate here...
        else {
            let mut buf: PathBuf = var_os("HOMEDRIVE")
                .map(|x| x.into())
                .unwrap_or_else(|| "C:\\".into());
            buf.push("Documents and Settings");
            ret.push(buf);
        }
    }
    else if cfg!(target_os = "macos") {
        // we should use NSSearchPathForDirectoriesInDomains(...), but that's
        // for later, maybe
        let mut buf: PathBuf = var_os("HOME")
            .expect("HOME environment variable not set!")
            .into();
        buf.push("Library");
        buf.push("Application Support");
        ret.push(buf);
    }
    else {
        // (use `#[cfg]` blocks instead of `if cfg!` because the absence of
        // UNIX `OsStrExt` would cause errors otherwise)
        #[cfg(target_family = "unix")] {
            // let's implement the XDG Base Directory Specification ... again!
            // Start with `XDG_CONFIG_DIRS`...
            let config_dirs = var_os("XDG_CONFIG_DIRS")
                .unwrap_or("/etc/xdg".into());
            for config_dir in config_dirs.as_bytes().split(|x| *x == b':') {
                if config_dir.len() == 0 { continue }
                ret.push(OsStr::from_bytes(config_dir).into());
            }
            // Then `XDG_CONFIG_HOME`...
            if let Some(config_home) = var_os("XDG_CONFIG_HOME") {
                ret.push(config_home.into());
            }
            else if let Some(home) = var_os("HOME") {
                let mut buf: PathBuf = home.into();
                buf.push(".config");
                ret.push(buf);
            }
        }
        #[cfg(not(target_family = "unix"))]
        if let Some(home) = var_os("HOME") {
            let mut buf = home.into();
            buf.push(".config");
            ret.push(buf);
        }
    }
    // OS-generic: if no paths provided, make one up as a last ditch effort
    // (probably relevant to Emscripten/WASM/embedded?)
    if ret.len() == 0 {
        ret.push("/home/.config".into());
    }
    // Now... what we're interested in is a Tsong *subdirectory* in all those
    // directories!
    for p in ret.iter_mut() {
        p.push("Tsong");
    }
    ret
}

/// Call the given closure once for each configuration file with the given name
/// found. Starts with the most general file, ends with the most specific one.
/// Any existing configuration values should be overridden by subsequent calls.
pub fn for_each_config_file<F: FnMut(&Path) -> anyhow::Result<()>>(name: &str,
                                                                   mut f: F)
    -> anyhow::Result<()> {
    let backed_up_name = name.to_owned() + BACKUP_SUFFIX;
    for path in CONFIG_PATHS.iter() {
        let result = {
            let mut path_buf = path.to_owned();
            path_buf.push(name);
            if path_buf.exists() {
                f(&path_buf).context("Error while reading a config file")
            }
            else {
                path_buf.pop();
                path_buf.push(&backed_up_name);
                if path_buf.exists() {
                    f(&path_buf).context("Error while reading a backup config \
                                          file")
                }
                else { Ok(()) }
            }
        };
        if let Err(x) = result {
            eprintln!("WARNING: Error reading configuration file:\n{:?}\n", x);
        }
    }
    Ok(())
}

/// Returns the "best path" for the given configuration file. This should only
/// be used when you are handling safe updates yourself, e.g. when the file is
/// an sqlite database.
pub fn get_config_file_path(name: &str) -> PathBuf {
    assert!(CONFIG_PATHS.len() > 0);
    let mut buf = CONFIG_PATHS[CONFIG_PATHS.len() - 1].to_owned();
    buf.push(name);
    buf
}

/// Represents a configuration file in the process of being updated. You may
/// treat this as a standard `File`, since it `Deref`s to one. However, there
/// is one additional concern. When you are finished updating the file, if the
/// updating process was successful, **you must call `finish()` in order for
/// the updates to take effect**. If the `Update` is dropped without `finish()`
/// being called, **the updates will be lost**.
pub struct Update {
    inner: File,
    neu_path: PathBuf,
    final_path: PathBuf,
    backup_path: PathBuf,
    finished: bool,
}

impl Deref for Update {
    type Target = File;
    fn deref(&self) -> &File { &self.inner }
}

impl DerefMut for Update {
    fn deref_mut(&mut self) -> &mut File { &mut self.inner }
}

impl Drop for Update {
    fn drop(&mut self) {
        if self.finished { return }
        if let Err(x) = fs::remove_file(&self.neu_path) {
            if cfg!(debug_assertions) {
                eprintln!("WARNING: Couldn't delete a config file whose \
                           update process aborted! {:?}", x);
            }
        }
    }
}

impl Update {
    /// Call this when you have finished writing the file, and experienced no
    /// errors in the process. This will flush the file, back up the old
    /// version (if any), and move the new one into place.
    pub fn finish(mut self) -> anyhow::Result<()> {
        assert!(!self.finished);
        self.flush()?;
        self.finished = true;
        // make local copies of these, since we are about to drop ourselves
        let backup_path = self.backup_path.clone();
        let neu_path = self.neu_path.clone();
        let final_path = self.final_path.clone();
        // close the file (some OSes won't let us rename an open file)
        drop(self);
        // try backing up the original file... but ignore an error in that
        // process
        let _ = fs::rename(&final_path, &backup_path);
        // now move the new file into place
        Ok(fs::rename(&neu_path, &final_path)?)
    }
}

/// Opens a configuration file for writing. If successful, returns an
/// [`Update`][1], which `Deref`s to a File. `Update` provides stronger
/// guarantees about transactional integrity than simply opening and writing
/// the file in place. You must call `.finish()` on successful write, or the
/// changes will be lost.
///
/// [1]: struct.Update.html
pub fn open_for_write(name: &str) -> anyhow::Result<Update> {
    let src = &CONFIG_PATHS[CONFIG_PATHS.len() - 1];
    let mut neu_path = src.to_owned();
    neu_path.push(name.to_owned() + NEW_SUFFIX);
    let inner = File::create(&neu_path)
        .or_else(|_| {
            fs::create_dir_all(src)?;
            File::create(&neu_path)
        })
        .context("Couldn't create configuration file")?;
    let mut final_path = src.to_owned();
    final_path.push(name);
    let mut backup_path = src.to_owned();
    backup_path.push(name.to_owned() + BACKUP_SUFFIX);
    Ok(Update { inner, neu_path, final_path, backup_path, finished: false })
}

/// Tries to create the configuration directory if it doesn't exist.
pub fn try_create_config_dir() -> std::io::Result<()> {
    let src = &CONFIG_PATHS[CONFIG_PATHS.len() - 1];
    fs::create_dir_all(src)
}

/// Opens the most specific available configuration file with the given name,
/// if one is found. Returns `Ok(None)` if no configuration file was found.
pub fn open_best_for_read(name: &str) -> anyhow::Result<Option<File>> {
    let backed_up_name = name.to_owned() + BACKUP_SUFFIX;
    for path in CONFIG_PATHS.iter().rev() {
        let mut path_buf = path.to_owned();
        path_buf.push(name);
        if path_buf.exists() {
            return File::open(path_buf).map(|x| Some(x))
                .context("Error while reading a config file")
        }
        else {
            path_buf.pop();
            path_buf.push(&backed_up_name);
            if path_buf.exists() {
                return File::open(path_buf).map(|x| Some(x))
                    .context("Error while reading a backup config file")
            }
        }
    }
    Ok(None)
}

