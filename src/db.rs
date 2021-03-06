//! This module handles the physical database, i.e. the backing store for
//! song, file, and playlist information.
use crate::*;

use log::{debug, info, error};
use std::{
    cell::RefCell,
    collections::BTreeMap,
    sync::Mutex,
};

use anyhow::anyhow;
use lazy_static::lazy_static;
use rusqlite::{
    Connection,
    params,
};
use serde_json as json;

lazy_static! {
    static ref DATABASE: Mutex<Option<RefCell<Connection>>>
        = Mutex::new(None);
}

pub fn open_database() -> anyhow::Result<()> {
    let mut database_lock = DATABASE.lock().unwrap();
    assert!(database_lock.is_none());
    let db_path = config::get_config_file_path("Database.sqlite3");
    let database = Connection::open(&db_path)
        .map_err(anyhow::Error::new)
        .or_else(|_| {
            config::try_create_config_dir()?;
            Connection::open(&db_path)
                .map_err(anyhow::Error::new)
        })?;
    let user_version = database.query_row
        ("SELECT user_version FROM pragma_user_version;", rusqlite::NO_PARAMS,
         |row| row.get(0));
    match user_version? {
        0 => {
            database.execute_batch(include_str!("sql/schema.sql"))?;
            debug!("Initialized database from schema.");
        },
        1 => {
            // TODO: prompt user for upgrades? try to back up the file?
            info!("Updating database from schema version 1.");
            database.execute_batch(include_str!("sql/update_1_to_2.sql"))?;
            database.execute_batch(include_str!("sql/update_2_to_3.sql"))?;
        },
        2 => {
            info!("Updating database from schema version 2.");
            database.execute_batch(include_str!("sql/update_2_to_3.sql"))?;
        },
        3 => {
            debug!("Database did not require initialization.");
        },
        _ => return Err(anyhow!("Unknown database format version. (Was it \
                                 created by a newer version of Tsong?)")),
    }
    let mut get_files = database.prepare("SELECT id, size, duration, \
                                          relative_paths \
                                          FROM PhysicalFiles;")?;
    let mut rows = get_files.query(rusqlite::NO_PARAMS)?;
    while let Some(row) = rows.next()? {
        let id: Vec<u8> = row.get_unwrap(0);
        let size: i64 = row.get_unwrap(1);
        let duration: i64 = row.get_unwrap(2);
        let relative_paths: String = row.get_unwrap(3);
        let id = FileID::from_bytes(&id[..])?;
        let size = size as u64;
        let duration = duration as u32;
        let relative_paths = json::from_str(&relative_paths)?;
        physical::add_file_from_db(id, size, duration, relative_paths);
    }    
    drop(rows);
    drop(get_files);
    let mut get_songs = database.prepare("SELECT id, user_metadata, \
                                          physical_files, similarity_recs, \
                                          duration \
                                          FROM LogicalSongs;")?;
    let mut rows = get_songs.query(rusqlite::NO_PARAMS)?;
    while let Some(row) = rows.next()? {
        let id: i64 = row.get_unwrap(0);
        let user_metadata: String = row.get_unwrap(1);
        let physical_files: Vec<u8> = row.get_unwrap(2);
        let similarity_recs: Option<String> = row.get_unwrap(3);
        let duration: Option<i64> = row.get_unwrap(4);
        let id = SongID::from_inner(id as u64);
        let user_metadata = json::from_str(&user_metadata)?;
        let physical_files = physical_files.chunks_exact(physical::ID_SIZE)
            .map(FileID::from_bytes).map(|x| x.unwrap()).collect();
        let similarity_recs = match similarity_recs {
            Some(x) => json::from_str(&x)?,
            None => None,
        };
        let duration = duration.unwrap_or(296) as u32;
        logical::add_song_from_db(id, user_metadata, physical_files,
                                  similarity_recs, duration);
    }
    drop(rows);
    drop(get_songs);
    let mut get_playlists = database.prepare("SELECT id, parent_id, \
                                              parent_order, name, rule_code, \
                                              manually_added_ids, columns, \
                                              sort_order, shuffled, playmode \
                                              FROM Playlists;")?;
    let mut rows = get_playlists.query(rusqlite::NO_PARAMS)?;
    while let Some(row) = rows.next()? {
        let id: i64 = row.get_unwrap(0);
        let parent_id: Option<i64> = row.get_unwrap(1);
        let parent_order: Option<i64> = row.get_unwrap(2);
        let name: String = row.get_unwrap(3);
        let rule_code: Option<String> = row.get_unwrap(4);
        let manually_added_ids: Option<String> = row.get_unwrap(5);
        let columns: Option<String> = row.get_unwrap(6);
        let sort_order: Option<String> = row.get_unwrap(7);
        let shuffled: Option<bool> = row.get_unwrap(8);
        let playmode: Option<i64> = row.get_unwrap(9);
        // massage the returned data
        let id = PlaylistID::from_inner(id as u64);
        let parent_id = parent_id.map(|x| x as u64)
            .map(PlaylistID::from_inner);
        let parent_order = parent_order.map(|x| x as u64).unwrap_or(u64::MAX);
        let rule_code = rule_code.unwrap_or_else(String::new);
        let manually_added_ids = match manually_added_ids {
            Some(x) =>
                json::from_str::<Vec<u64>>(&x)?
                    .into_iter().map(SongID::from_inner).collect(),
            None => vec![],
        };
        let columns = match columns {
            Some(x) => json::from_str(&x)?,
            None => playlist::DEFAULT_COLUMNS.clone(),
        };
        let sort_order = match sort_order {
            Some(x) => json::from_str(&x)?,
            None => playlist::DEFAULT_SORT_ORDER.clone(),
        };
        let shuffled = shuffled.unwrap_or(false);
        let playmode = Playmode::from_db_value(playmode.unwrap_or(0));
        playlist::add_playlist_from_db(id, parent_id, parent_order, name,
                                       rule_code, shuffled, playmode,
                                       manually_added_ids, columns,
                                       sort_order);
    }
    drop(rows);
    drop(get_playlists);
    *database_lock = Some(RefCell::new(database));
    drop(database_lock);
    playlist::rebuild_children();
    Ok(())
}

pub fn create_playlist(new_playlist_name: &str, new_parent_order: u64)
-> anyhow::Result<PlaylistID> {
    // well, this is a heckin' tangle
    // TODO: untangle this and its ilk
    let lock = DATABASE.lock();
    let database = lock.as_ref().unwrap().as_ref().unwrap().borrow_mut();
    database.execute("INSERT INTO Playlists(name, parent_order) \
                      VALUES (?, ?);",
                     params![new_playlist_name, new_parent_order as i64])?;
    Ok(PlaylistID::from_inner(database.last_insert_rowid() as u64))
}

pub fn update_playlist_name(id: PlaylistID, new_name: &str) {
    let lock = DATABASE.lock();
    let database = lock.as_ref().unwrap().as_ref().unwrap().borrow_mut();
    dbtry(database.execute("UPDATE Playlists SET name = ? WHERE id = ?;",
                           params![new_name, id.as_inner() as i64]));
}

pub fn update_playlist_rule_code(id: PlaylistID, new_code: &str) {
    let new_code = if new_code == "" { None } else { Some(new_code) };
    let lock = DATABASE.lock();
    let database = lock.as_ref().unwrap().as_ref().unwrap().borrow_mut();
    dbtry(database.execute("UPDATE Playlists SET rule_code = ? WHERE id = ?;",
                           params![new_code, id.as_inner() as i64]));
}

pub fn update_playlist_rule_code_and_columns(id: PlaylistID, new_code: &str,
                                             columns: &[playlist::Column]) {
    let new_code = if new_code == "" { None } else { Some(new_code) };
    let columns = json::to_string(columns).unwrap();
    let lock = DATABASE.lock();
    let database = lock.as_ref().unwrap().as_ref().unwrap().borrow_mut();
    dbtry(database.execute("UPDATE Playlists SET rule_code = ?, columns = ? \
                            WHERE id = ?;",
                           params![new_code, columns, id.as_inner() as i64]));
}

pub fn update_playlist_manually_added_songs(id: PlaylistID, songs: &[SongID]) {
    let songs = json::to_string(&songs.iter().map(SongID::as_inner).collect()
                                as &Vec<u64>).unwrap();
    let lock = DATABASE.lock();
    let database = lock.as_ref().unwrap().as_ref().unwrap().borrow_mut();
    dbtry(database.execute("UPDATE Playlists SET manually_added_ids = ? \
                            WHERE id = ?;",
                           params![songs, id.as_inner() as i64]));
}

pub fn update_playlist_shuffled(id: PlaylistID, shuffled: bool) {
    let lock = DATABASE.lock();
    let database = lock.as_ref().unwrap().as_ref().unwrap().borrow_mut();
    dbtry(database.execute("UPDATE Playlists SET shuffled = ? \
                            WHERE id = ?;",
                           params![shuffled, id.as_inner() as i64]));
}

pub fn update_playlist_playmode(id: PlaylistID, playmode: Playmode) {
    let lock = DATABASE.lock();
    let database = lock.as_ref().unwrap().as_ref().unwrap().borrow_mut();
    dbtry(database.execute("UPDATE Playlists SET playmode = ? \
                            WHERE id = ?;",
                           params![playmode.to_db_value(),
                                   id.as_inner() as i64]));
}

pub fn update_playlist_parent_order(id: PlaylistID, order: u64) {
    let lock = DATABASE.lock();
    let database = lock.as_ref().unwrap().as_ref().unwrap().borrow_mut();
    dbtry(database.execute("UPDATE Playlists SET parent_order = ? \
                            WHERE id = ?;",
                           params![order as i64, id.as_inner() as i64]));
}

pub fn update_playlist_parent_id(id: PlaylistID,
                                 parent: Option<PlaylistID>) {
    let lock = DATABASE.lock();
    let database = lock.as_ref().unwrap().as_ref().unwrap().borrow_mut();
    dbtry(database.execute("UPDATE Playlists SET parent_id = ? \
                            WHERE id = ?;",
                           params![parent.map(|x| x.as_inner() as i64),
                                   id.as_inner() as i64]));
}

pub fn update_playlist_parent_id_and_order(id: PlaylistID,
                                           parent: Option<PlaylistID>,
                                           order: u64) {
    let lock = DATABASE.lock();
    let database = lock.as_ref().unwrap().as_ref().unwrap().borrow_mut();
    dbtry(database.execute("UPDATE Playlists SET parent_id = ?, \
                            parent_order = ? WHERE id = ?;",
                           params![parent.map(|x| x.as_inner() as i64),
                                   order as i64,
                                   id.as_inner() as i64]));
}

pub fn update_playlist_sort_order_and_disable_shuffle(id: PlaylistID,
                                                      sort_order: &[(String,
                                                                     bool)]) {
    
    let lock = DATABASE.lock();
    let database = lock.as_ref().unwrap().as_ref().unwrap().borrow_mut();
    let sort_order = json::to_string(sort_order).unwrap();
    dbtry(database.execute("UPDATE Playlists SET shuffled = 0, \
                            sort_order = ? WHERE id = ?;",
                           params![sort_order, id.as_inner() as i64]));
}

pub fn update_playlist_columns(id: PlaylistID, columns: &[playlist::Column]) {
    let lock = DATABASE.lock();
    let database = lock.as_ref().unwrap().as_ref().unwrap().borrow_mut();
    let columns = json::to_string(columns).unwrap();
    dbtry(database.execute("UPDATE Playlists SET columns = ? WHERE id = ?;",
                           params![columns, id.as_inner() as i64]));
}

pub fn delete_playlist(id: PlaylistID) {
    let lock = DATABASE.lock();
    let database = lock.as_ref().unwrap().as_ref().unwrap().borrow_mut();
    dbtry(database.execute("DELETE FROM Playlists WHERE id = ?;",
                           params![id.as_inner() as i64]));
}

pub fn add_file(id: &FileID, size: u64,
                duration: u32, relative_paths: &Vec<String>) {
    let relative_paths = json::to_string(relative_paths).unwrap();
    let lock = DATABASE.lock();
    let database = lock.as_ref().unwrap().as_ref().unwrap().borrow_mut();
    dbtry(database.execute("INSERT INTO PhysicalFiles \
                            (id, size, duration, relative_paths) \
                            VALUES (?, ?, ?, ?);",
                           params![&id.as_bytes()[..],
                                   size as i64, duration as i64,
                                   relative_paths]));
}

pub fn update_file_relative_paths(id: &FileID, paths: &Vec<String>) {
    let paths = json::to_string(paths).unwrap();
    let lock = DATABASE.lock();
    let database = lock.as_ref().unwrap().as_ref().unwrap().borrow_mut();
    dbtry(database.execute("UPDATE PhysicalFiles SET relative_paths = ? \
                            WHERE id = ?;",
                           params![paths, &id.as_bytes()[..]]));
}

pub fn add_song(user_metadata: &BTreeMap<String, String>,
                physical_files_in: &Vec<FileID>,
                similarity_recs: &[logical::SimilarityRec],
                duration: u32)
-> anyhow::Result<SongID> {
    let user_metadata = json::to_string(user_metadata).unwrap();
    let mut physical_files: Vec<u8> = Vec::with_capacity(physical_files_in
                                                         .len()
                                                         * physical::ID_SIZE);
    for id in physical_files_in.iter() {
        physical_files.extend_from_slice(id.as_bytes());
    }
    let lock = DATABASE.lock();
    let database = lock.as_ref().unwrap().as_ref().unwrap().borrow_mut();
    database.execute("INSERT INTO LogicalSongs \
                      (user_metadata, physical_files, similarity_recs, \
                      duration) \
                      VALUES (?, ?, ?, ?);",
                     params![user_metadata, physical_files,
                             json::to_string(similarity_recs).unwrap(),
                             duration])?;
    Ok(SongID::from_inner(database.last_insert_rowid() as u64))
}

pub fn update_song_physical_files(id: SongID, physical_files_in:&Vec<FileID>){
    let mut physical_files: Vec<u8>
        = Vec::with_capacity(physical_files_in .len() * physical::ID_SIZE);
    for id in physical_files_in.iter() {
        physical_files.extend_from_slice(id.as_bytes());
    }
    let lock = DATABASE.lock();
    let database = lock.as_ref().unwrap().as_ref().unwrap().borrow_mut();
    dbtry(database.execute("UPDATE LogicalSongs SET physical_files = ? \
                            WHERE id = ?;",
                           params![physical_files, id.as_inner() as i64]));
}

pub fn update_song_physical_files_and_similarity_recs
    (id: SongID, physical_files_in: &Vec<FileID>,
     similarity_recs_in: &[logical::SimilarityRec]) {
    let mut physical_files: Vec<u8>
        = Vec::with_capacity(physical_files_in .len() * physical::ID_SIZE);
    for id in physical_files_in.iter() {
        physical_files.extend_from_slice(id.as_bytes());
    }
    let similarity_recs = json::to_string(similarity_recs_in).unwrap();
    let lock = DATABASE.lock();
    let database = lock.as_ref().unwrap().as_ref().unwrap().borrow_mut();
    dbtry(database.execute("UPDATE LogicalSongs SET physical_files = ?, \
                            similarity_recs = ? \
                            WHERE id = ?;",
                           params![physical_files, similarity_recs,
                                   id.as_inner() as i64]));
}

pub fn update_song_similarity_recs
    (id: SongID, similarity_recs_in: &[logical::SimilarityRec]) {
    let similarity_recs = json::to_string(similarity_recs_in).unwrap();
    let lock = DATABASE.lock();
    let database = lock.as_ref().unwrap().as_ref().unwrap().borrow_mut();
    dbtry(database.execute("UPDATE LogicalSongs SET similarity_recs = ? \
                            WHERE id = ?;",
                           params![similarity_recs, id.as_inner() as i64]));
}

pub fn update_song_metadata(id: SongID, metadata: &BTreeMap<String, String>) {
    let metadata = json::to_string(metadata).unwrap();
    let lock = DATABASE.lock();
    let database = lock.as_ref().unwrap().as_ref().unwrap().borrow_mut();
    dbtry(database.execute("UPDATE LogicalSongs SET user_metadata = ? \
                            WHERE id = ?;",
                           params![metadata, id.as_inner() as i64]));
}

pub fn update_song_duration(id: SongID, duration: u32) {
    let lock = DATABASE.lock();
    let database = lock.as_ref().unwrap().as_ref().unwrap().borrow_mut();
    dbtry(database.execute("UPDATE LogicalSongs SET duration = ? \
                            WHERE id = ?;",
                           params![duration as i64, id.as_inner() as i64]));
}

/// If a database error occurred, log it and return nothing. Otherwise, return
/// the returned value.
fn dbtry<X>(x: rusqlite::Result<X>) -> Option<X> {
    match x {
        Err(x) => {
            error!("Database error: {:?}", x);
            None
        },
        Ok(x) => Some(x),
    }
}
