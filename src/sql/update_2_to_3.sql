BEGIN TRANSACTION;
CREATE TABLE NewPhysicalFiles(
       id BINARY(16) PRIMARY KEY,
       size INTEGER NOT NULL,
       duration INTEGER NOT NULL,
       relative_paths BLOB NOT NULL
);
INSERT INTO NewPhysicalFiles(id, size, duration, relative_paths)
SELECT id, size, duration, relative_paths FROM PhysicalFiles;
DROP TABLE PhysicalFiles;
ALTER TABLE NewPhysicalFiles RENAME TO PhysicalFiles;
ALTER TABLE LogicalSongs ADD COLUMN similarity_recs BLOB;
COMMIT;
PRAGMA user_version = 3;
