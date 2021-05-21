This is a cross-platform, open source music player program. It is designed around a non-destructive paradigm; your original music files are never modified or rearranged without your permission. **It is currently alpha software,** but is complete enough to serve as a media player. And since it's written in Rust, it's rock stable.

(Actually, it depends rather heavily on FFmpeg, which has sometimes been known to have crashes. Also, a few code paths will panic, which is mostly indistinguishable from a crash. If you run into either one, please [file an issue](https://github.com/SolraBizna/tsong/issues).)

# Features

- Cross-platform
    - Linux
    - macOS
    - Windows
    - ...and anywhere else Rust, GTK+, and PortAudio can be had!
- Near-universal format support
    - AIFF
    - FLAC
    - MP3
    - MP4/AAC
    - Ogg Vorbis
    - WAV
    - WMA
    - ...and anything else FFmpeg supports!
- Playlists can be populated automatically by [rules](#rules)
- Can treat different recordings/encodings of the same song as one song
- Arbitrary, user-specified metadata for any song
    - Never moves or edits the original files (all metadata is stored in a central database)
    - Customizable metadata import via Lua scripting (see [the example script](src/lua/import.lua.example))
- Supports [MPRIS](https://wiki.archlinux.org/title/MPRIS) for external control
- Limited support for loop metadata
- Easy on the CPU, easy on the battery

# Rules

Playlists may be populated using short [Lua 5.4](https://www.lua.org/) expressions as rules. All metadata in the song is present as variables in the environment; any missing metadata is present too, as empty strings (for convenience). Metadata that isn't valid Lua identifiers must be fetched by accessing `_ENV`, as in the rather clumsy `_ENV["#tracks"]`.

A future version will add an iTunes-like purely graphical editor for rules.

## Extra functions

You have access to all of the syntax and the string manipulation functions normally present in Lua, including conjunctions with `and` and `or` and negations with `not`. You also have access to the following:

- `<tag>:contains "wat"`  
  Evalutes to true if `<tag>` contains the text "wat". (This is a wrapper around `string.find(<tag>, "wat", 1, true)`.)
- `<tag>:starts_with "wat"`  
  Evaluates to true if `<tag>` starts with the text "wat".
- `<tag>:ends_with "wat"`  
  Evaluates to true if `<tag>` ends with the text "wat".
- `<tag>:set()`  
  Evaluates to true if `<tag>` exists and is set to a value other than "0".
- `<tag>:unset()`  
  Evaluates to true if `<tag>` does not exist, or is set to "0".

## Examples

- `artist:contains "Loreena McKennitt"`  
  All of your songs by Loreena McKennitt, including a hypothetical song by "Daft Punk, Loreena McKennitt, and The London Philharmonic Orchestra", since that contains the text "Loreena McKennitt". (Anybody with a song that fits this description, please tell me where I can buy it!)
- `album:contains "Twilight Princess" and not title:contains "Zant"`  
  All of your Twilight Princess songs, except the fifty-seven different versions of Zant's boss theme and any other song that's Zant-related.
- `(artist:contains "Strong Bad" and not artist:contains "LiveStrong Bad Advice Podcast") or artist:contains "Homestar Runner"`  
  All of your songs by either Strong Bad or Homestar Runner, including (for example) a hypothetical song by "Strong Bad & Homestar Runner". A song where "LiveStrong Bad Advice Podcast" is the artist, which would normally be allowed as it contains the text "Strong Bad", would be excluded here... *unless* the artist also includes "Homestar Runner".
- `_ENV["track#"]:set() and _ENV["#tracks"]:set() and tonumber(_ENV["track#"]) / tonumber(_ENV["#tracks"]) <= 0.5`  
  This rather convoluted example will match all of your songs that have metadata indicating both a track number in an album and the number of tracks in their album, and are from the first half of the album.
- `any`  
  `true`  
  Either of these expressions always evaluates to a true value, so every song will be accepted. (Actually, any identifier on its own will always evaluate to a true value, which is why `:set()` exists.)
- `unchecked:set()`  
  By default, when importing a new song's metadata, Tsong adds a special `unchecked` metadata tag to indicate that you, the human operating Tsong, haven't looked over the metadata yourself yet. Once you've pruned, tuned, and filled in the metadata as you see fit, you are supposed to remove that tag (or set it to "0"). This rule will accept any songs whose metadata you haven't checked.

(The first time you start Tsong, it automatically creates playlists with the latter two rules for you, named "All Songs" and "Unchecked Songs".)

Note that Lua string handling is case sensitive and very particular. Using the Loreena McKennitt rule as an example, if you have a song with the artist tag "Loreena McKennit" (note the typoed double-t at the end), it won't go in that playlist. Similarly, "Loreena Mckennitt" (lowercase k) won't either.

# Compiling

To compile Tsong, you will need a Rust compiler, and development files for GTK+ 3.16 or later, `libsoxr`, and PortAudio v19. [Here are some quick start instructions](https://www.rust-lang.org/learn/get-started) for getting a Rust compiler. For the other requirements, obtain them by whatever means your build environment requires.

```sh
git clone https://github.com/SolraBizna/tsong
cd tsong
cargo build --release
```

Now you have `target/bin/tsong`, ready to run.

If you are running on Windows or macOS (or any other platform on which DBus isn't routinely available), you probably need to run `cargo build --release --no-default-features` instead, to disable MPRIS support. The library we use for MPRIS support will panic if DBUS isn't available.

# Legalese

Tsong is licensed under [the MIT license](COPYING.md), and is copyright ©2021 Solra Bizna.
