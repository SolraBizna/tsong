-- This Lua 5.4 script is applied to any songs that are newly imported into
-- your library. It controls how raw, format-dependent metadata is mapped to
-- the metadata Tsong uses. You can edit this freely. If you screw it up too
-- badly, just delete the file and Tsong will put the default script in its
-- place the next time you start it up.
--
-- For more information about the Lua scripting language and its syntax, see:
-- <https://www.lua.org/manual/5.4/>
--
-- Global variables available:
-- - `inmeta`: The metadata returned from FFMPEG, i.e. the metadata in the
--   file.
-- - `outmeta`: The metadata that will be in the song. If this script is called
--   from the "Re-import Metadata" button in the metadata editor, this will be
--   pre-populated with the existing metadata of the logical song. Otherwise,
--   it will be empty.
-- - `filename`: The filename of the physical file.
-- - `filenames`: An array of (unique) filenames of the physical file, in case
--   the same file appears more than once in your library.
-- - `path`: The (OS-dependent) absolute path of the physical file, including
--   the filename.
-- - `paths`: As with `filenames`, this is an array of paths, in case the same
--   file appears more than once in your library.
-- - `file_id`: The unique hash of the original file, as hexadecimal digits.
-- - `song_id`: Only available when called from the "Re-import Metadata"
--   button. This is the "logical song ID", a unique number representing this
--   particular song in the library.
--
-- The following convenience functions are provided:
-- - `consume_tag(key)`  
--   If `key` is present in `inmeta` and non-empty, returns its value and
--   removes it from `inmeta`.
-- - `strip_raw_outmeta()`  
--   Removes any keys from `outmeta` that begin with `raw_` (i.e. were probably
--   added by `set_raw_outmeta()`.
-- - `set_raw_outmeta()`  
--   Prepends `raw_` to all keys that remain in `inmeta`, and brings them over
--   to `outmeta`.
-- - `remap_id3v2_tags()`  
--   Try to map raw ID3v2 tags that slipped through into more human-readable
--   equivalents.
--
-- You are free to use whatever strings for metadata keys and values that you
-- want. However, there are some caveats:
-- - Metadata should always be valid UTF-8, preferably in normal form D. (You
--   don't need to worry about this unless your script is doing some really
--   interesting things.)
-- - The "duration" metadata key is special. It always contains the duration of
--   the song in seconds, and cannot be changed.
-- - The "song_id" metadata key is special. It always contains the logical song
--   ID for the song, and cannot be changed.
-- - The "loop_start" and "loop_end" metadata keys may contain timestamps in
--   seconds, possibly with a decimal part (using "." as the radix separator).
--   If they are present and valid, they are used when looping a single track.

-- Comment out the following line if you want to preserve previously-set
-- metadata on the song:
outmeta = {}

-- In case the above line was commented out, strip any "raw_*" metadata that
-- might have been left over from a previous scan.
strip_raw_outmeta()

-- Mark the song as being "unchecked", so that the user can manually check to
-- make sure all the metadata makes sense.
outmeta.unchecked = "1"

-- Before we proceed, map ID3v2 tags to human-readable equivalents. FFMPEG
-- tries to do this for us, but it doesn't try very hard, and some "raw tags"
-- slip through.
--
-- The resulting human readable names will all start with "id3v2_". Later code
-- may bring some or all of these tags into the fold, but most of them will end
-- up being "raw_id3v2_" tags when the chips fall.
remap_id3v2_tags()

-- Some metadata keys can be trivially passed through:
for _, key in ipairs {
   "album", "album_artist", "artist", "comment", "composer", "encoded_by",
   "encoder", "engineer", "genre", "language", "performer", "publisher",
   "title", "year",
   -- not standard tags, but used in some "looping" Ogg Vorbis files
   "loop_start", "loop_end",
} do
   -- for "album", consume "album" or "Album" or "ALBUM"
   local value = consume_tag(key)
      or consume_tag(key:sub(1,1):upper() .. key:sub(2,-1))
      or consume_tag(key:upper())
      or consume_tag("id3v2_" .. key)
   if value then
      outmeta[key] = value
   end
end

-- Some metadata keys that represent numbers... Tsong prefers to end such
-- metadata with `#`. Also, some (but not all) conventions for number
-- metadata include both the index and the count in the same metadata tag.
-- Tsong prefers to split the two.
for _, base in ipairs { "disc", "track" } do
   local value = consume_tag(base)
      or consume_tag(base:sub(1,1):upper() .. base:sub(2,-1))
      or consume_tag(base:upper())
      or consume_tag("id3v2_" .. base)
   if value then
      local index,count = value:match("^0*([0-9]+)[ \t]*/[ \t]*(0*[0-9]+)$")
      if index then
         outmeta[base.."#"] = index
         outmeta["#"..base.."s"] = count
      else
         local index = value:match("^0*([0-9]+)$")
         if index then
            outmeta[base.."#"] = index
         else
            -- put it back in inmeta, let it become "raw_*"
            inmeta[base] = value
         end
      end
   end
end

-- Some songs have a "date" metadata value which is just a year. Others have
-- one that's a full-blown ISO timestamp. Try to turn the former into a "year",
-- and extract a "year" from the latter.
local date = consume_tag "date" or consume_tag "Date" or consume_tag "DATE"
if date then
   local bare_year = date:match("^[0-9]+$")
   if bare_year then
      outmeta.year = bare_year
   else
      local sub_year = date:match("^[0-9]+%-")
      if sub_year then
         outmeta.year = sub_year
      end
      outmeta.date = date
   end
end

-- Some songs may have two-digit years. Try to turn those into four-digit ones.
if outmeta.year and #outmeta.year == 2 then
   if outmeta.year:sub(1,1) == "0" then
      -- probably a song from 200x
      outmeta.year = "20" .. outmeta.year
   else
      -- probably a song from 19xx
      outmeta.year = "19" .. outmeta.year
   end
end

-- If the song doesn't already have a title, try to make one from its filename.
if not outmeta.title then
   -- Strip the file extension, if any
   local filename = filename:gsub("%.[^.]+$", "")
   -- Strip any leading or trailing spaces
   filename = filename:gsub("^ +",""):gsub(" +$","")
   -- Try to extract a track number, while we're at it
   if not outmeta["track#"] then
      local number, rest = filename:match("^0*([0-9]+)[-_ .]+(.+)$")
      if number then
         outmeta["track#"] = number
         filename = rest
      end
   end
   outmeta.title = filename
end

-- A very old version of the ID3 metadata standard stored a 30-byte comment.
-- The next version stored a 28-byte comment and used what had previously been
-- the last two bytes of the comment to store a track number. Try to clean up
-- after cases where a tool might have misinterpreted the latter as the former.
if outmeta.comment and outmeta.comment:sub(-2,-2) == "\0" then
   local comment_track_number = outmeta.comment:byte(-1)
   outmeta.comment = outmeta.comment:sub(1,-3):gsub("[ \t]+$", "")
   -- Only actually USE the extracted track number if there isn't one already
   -- set AND the track number was actually set in the broken comment
   if not outmeta["track#"] and comment_track_number > 0 then
      outmeta["track#"] = ("%i"):format(comment_track_number)
   end
end

-- ID3v1 stores genre as a number. Try to map such genres to their American
-- English names.
if outmeta.genre == "255" then outmeta.genre = nil end
if outmeta.genre and #outmeta.genre <= 3
   and outmeta.genre:match("0*[0-9]+") then
   -- This list is taken from the Wikipedia article on ID3, as of 2021-03-04.
   -- Inconsistent capitalization, spacing, and punctuation has been fixed, and
   -- the infamous racist slur (#133) has been omitted completely.
   local GENRES<const> = {[0]="Blues", "Classic Rock", "Country", "Dance",
      "Disco", "Funk", "Grunge", "Hip-Hop", "Jazz", "Metal", "New Age",
      "Oldies", "Other", "Pop", "Rhythm and Blues", "Rap", "Reggae", "Rock",
      "Techno", "Industrial", "Alternative", "Ska", "Death Metal", "Pranks",
      "Soundtrack", "Euro-Techno", "Ambient", "Trip-Hop", "Vocal",
      "Jazz and Funk", "Fusion", "Trance", "Classical", "Instrumental", "Acid",
      "House", "Game", "Sound Clip", "Gospel", "Noise", "Alternative Rock",
      "Bass", "Soul", "Punk", "Space", "Meditative", "Instrumental Pop",
      "Instrumental Rock", "Ethnic", "Gothic", "Darkwave", "Techno-Industrial",
      "Electronic", "Pop-Folk", "Eurodance", "Dream", "Southern Rock",
      "Comedy", "Cult", "Gangsta", "Top 40", "Christian Rap", "Pop/Funk",
      "Jungle", "Native US", "Cabaret", "New Wave", "Psychadelic", "Rave",
      "Show Tunes", "Trailer", "Lo-Fi", "Tribal", "Acid Punk", "Acid Jazz",
      "Polka", "Retro", "Musical", "Rock'n'Roll", "Hard Rock",
      -- added after the fact
      "Folk", "Folk-Rock", "National Folk", "Swing", "Fast Fusion", "Bebop",
      "Latin", "Revival", "Celtic", "Bluegrass", "Avantgarde", "Gothic Rock",
      "Progressive Rock", "Psychedelic Rock", "Symphonic Rock", "Slow Rock",
      "Big Band", "Chorus", "Easy Listening", "Acoustic", "Humor", "Speech",
      "Chanson", "Opera", "Chamber Music", "Sonata", "Symphony", "Booty Bass",
      "Primus", "Porn Groove", "Satire", "Slow Jam", "Club", "Tango", "Samba",
      "Folklore", "Ballad", "Power Ballad", "Rhythmic Soul", "Freestyle",
      "Duet", "Punk Rock", "Drum Solo", "A Cappella", "Euro-House",
      "Dancehall", "Goa", "Drum and Bass", "Club-House", "Hardcore Techno",
      "Terror", "Indie", "BritPop", nil, "Polsk Punk", "Beat",
      "Christian Gangsta Rap", "Heavy Metal", "Black Metal", "Crossover",
      "Contemporary Christian", "Christian Rock", "Merengue", "Salsa",
      "Thrash Metal", "Anime", "J-Pop", "Synthpop", "Abstract", "Art Rock",
      "Baroque", "Bhangra", "Big Beat", "Breakbeat", "Chillout", "Downtempo",
      "Dub", "EBM", "Eclectic", "Electro", "Electroclash", "Emo",
      "Experimental", "Garage", "Global", "IDM", "Illbient", "Industro-Goth",
      "Jam Band", "Krautrock", "Leftfield", "Lounge", "Math Rock",
      "New Romantic", "Nu-Breakz", "Post-Punk", "Post-Rock", "Psytrance",
      "Shoegaze", "Space Rock", "Trop Rock", "World Music", "Neoclassical",
      "Audiobook", "Audio Theatre", "Neue Deutsche Welle", "Podcast",
      "Indie-Rock", "G-Funk", "Dubstep", "Garage Rock", "Psybient"
   }
   -- What's the difference between "Humor" and "Comedy"? Why does adding
   -- "Euro-" before a genre make a different genre? Why is "US Native"
   -- distinct from "Tribal" which is distinct from "Ethnic"? Why are "Primus"
   -- and "Leftfield" genres unto themselves?
   -- ...
   -- Who knows. I don't. I don't even use the "genre" tag, myself.
   local index = math.tointeger(outmeta.genre)
   if GENRES[index] then
      outmeta.genre = GENRES[index]
   end
end

-- Anything that's left in `inmeta`, add `raw_` to the beginning of the key,
-- and transfer it to `outmeta`, so that their values can later be manually
-- corrected or deleted.
set_raw_outmeta()
