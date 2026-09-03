# playlist-tidy

Playlist files coming out of ripping tools, old media players, and random
export scripts are rarely well-formed M3U. Common problems in files I've
actually hit:

- no `#EXTM3U` header, or one buried after some blank lines
- `#EXTINF` lines with a missing comma, a non-numeric duration, or no title
- entries with no `#EXTINF` at all
- Windows paths (`C:\Music\track.mp3`) mixed in with forward-slash ones
- CRLF line endings, a leading BOM, stray blank lines between entries

Most players tolerate this by guessing. `playlist-tidy` doesn't guess by
default - it tells you exactly what's wrong and where, so you can decide
whether the source file is worth fixing. Pass `--lenient` when you'd rather
have it repair what it can and just tell you what it changed.

It also reads PLS playlists (the `[playlist]` / `FileN=` / `TitleN=` /
`LengthN=` format some older rippers and Winamp-derived tools export) and
converts them to the same normalized M3U output. Format is picked from the
file extension, or by sniffing for a `[playlist]` header when reading from
stdin. Output is always M3U - there's no round-tripping back to PLS.

## Usage

Strict mode (default) rejects anything that isn't well-formed:

```
$ playlist-tidy my_mix.m3u
error: malformed #EXTINF at line 4: duration 'three' is not an integer
hint: pass --lenient to repair the file instead of failing
```

Lenient mode repairs what it can and reports every change on stderr:

```
$ playlist-tidy --lenient my_mix.m3u > clean.m3u
warning: repaired malformed #EXTINF at line 4 (duration 'three' is not an integer)
warning: repaired path at line 9 (path uses backslashes instead of forward slashes)
```

Read from stdin and write to a file:

```
$ cat exported.m3u8 | playlist-tidy --lenient -o clean.m3u -
```

Given this input:

```
#EXTINF:180,Radiohead - Idioteque

C:\Music\Kid A\04 idioteque.mp3
track_with_no_extinf.mp3
```

`playlist-tidy --lenient` produces:

```
#EXTM3U
#EXTINF:180,Radiohead - Idioteque
C:/Music/Kid A/04 idioteque.mp3
#EXTINF:-1,track_with_no_extinf.mp3
track_with_no_extinf.mp3
```

with warnings on stderr explaining the missing header, the backslash path,
and the synthesized `#EXTINF` for the last entry.

## What "strict" checks

- the file starts with `#EXTM3U`
- every entry line is preceded by a valid `#EXTINF:<duration>,<title>` line
- duration is an integer, `-1` (unknown/live) or higher
- title is non-empty
- local file paths use forward slashes (URLs are left alone)

Anything else - unrecognized `#`-directives like `#EXTGRP`, blank lines,
line ending style - is normalized without needing `--lenient`, since none
of that changes what the playlist means.

## Building

Standard library only, no external dependencies:

```
cargo build --release
```

## Status

Early. Handles single M3U/M3U8/PLS files passed as a path or via stdin. Not
yet handled: XSPF input, a `--check` mode, batch processing a directory,
verifying that referenced files actually exist on disk.

## License

MIT, see [LICENSE](LICENSE).
