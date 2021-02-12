PRAGMA user_version = 2;

CREATE TABLE PhysicalFiles(
       id BINARY(16) PRIMARY KEY,
       size INTEGER NOT NULL,
       duration INTEGER NOT NULL,
       relative_paths BLOB NOT NULL,
       raw_meta BLOB NOT NULL
);

CREATE TABLE LogicalSongs(
       id INTEGER PRIMARY KEY AUTOINCREMENT,
       user_metadata BLOB NOT NULL,
       physical_files BLOB NOT NULL,
       duration INTEGER
);

CREATE TABLE Playlists(
       id INTEGER PRIMARY KEY AUTOINCREMENT,
       parent_id INTEGER, -- NOTE: this is nullable!
       parent_order INTEGER,
       name BLOB NOT NULL,
       rule_code BLOB,
       manually_added_ids BLOB,
       columns BLOB,
       sort_order BLOB,
       shuffled BOOLEAN,
       playmode TINYINT
);

INSERT INTO Playlists(parent_order, name, rule_code)
       VALUES (0, 'All Songs', 'any'),
       (1, 'Unchecked Songs', 'unchecked:set()');
