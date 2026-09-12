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
XSPF playlists (the XML format, `<trackList>` / `<track>` / `<location>`),
converting either into the same normalized M3U output. Format is picked from
the file extension, or by sniffing the first non-blank line (`[playlist]`
for PLS, an XML declaration or `<playlist` root tag for XSPF) when reading
from stdin. Output is always M3U - there's no round-tripping back to PLS or
XSPF.

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

`--check` reports whether a file needs repair without writing anything
anywhere - useful in a script that walks a library and flags playlists worth
fixing:

```
$ playlist-tidy --check --lenient my_mix.m3u
warning: repaired path at line 9 (path uses backslashes instead of forward slashes)
playlist needs repair; rerun without --check to write the result
$ echo $?
1
```

A clean file (or, in strict mode, one with no errors) exits 0 and prints
nothing. `--check` can't be combined with `-o`, since it never produces
output to write.

## Verifying referenced files exist

`--verify` checks that every local path in the normalized output points at a
file that actually exists, resolved relative to the playlist's own directory
(not the current working directory) - URLs are skipped. It combines with
either normal or `--check` runs, and exits nonzero if anything is missing:

```
$ playlist-tidy --verify my_mix.m3u
warning: referenced file does not exist: songs/track_that_moved.mp3
$ echo $?
1
```

This only checks presence, not that the file is playable, and it never
touches the playlist's own repair status - a file can be clean and still
reference something missing, or vice versa.

## Relative paths when moving a playlist

When `-o` writes the result to a different directory than the input came
from, any relative (non-URL, non-absolute) path in the playlist is rewritten
so it still points at the same file from its new location:

```
$ cat my_library/mix.m3u
#EXTM3U
#EXTINF:180,Track
../shared/track.mp3
$ playlist-tidy -o backup/mix.m3u my_library/mix.m3u
warning: rewrote relative path '../shared/track.mp3' to '../my_library/shared/track.mp3'
```

Absolute paths and URLs are left alone, since they don't depend on the
playlist's own location. Resolution is lexical only - it doesn't touch the
filesystem or follow symlinks, so a rewritten path is only as good as the
directory structure it was computed from. Writing to the same directory the
input came from, or to stdout, leaves every path untouched. Batch mode does
the same per file, since mirroring the input's subdirectory layout under a
different output root doesn't otherwise preserve where relative paths
outside that layout point.

## Batch mode

Pass a directory instead of a file and every `.m3u`/`.m3u8`/`.pls`/`.xspf`
file under it (searched recursively) gets processed the same way a single
file would, one at a time. `-o` becomes the output directory - the input's
subdirectory layout is mirrored underneath it, with each file's extension
changed to `.m3u`:

```
$ playlist-tidy --lenient -o clean_library/ my_library/
warning: old/mix.pls: repaired path for entry 3 (path uses backslashes instead of forward slashes)
processed 14 files: 12 clean, 2 repaired, 0 failed
```

`-o` is required unless `--check` is given, since there's nowhere sensible
for multiple files' output to go otherwise. In strict mode a file that fails
is reported and counted, but doesn't stop the rest of the batch from being
processed. The exit code is nonzero if any file failed, or (under `--check`)
if any file needed repair.

```
$ playlist-tidy --check --lenient my_library/
warning: old/mix.pls: repaired path for entry 3 (path uses backslashes instead of forward slashes)
processed 14 files: 12 clean, 2 repaired, 0 failed
$ echo $?
1
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

Early. Handles single M3U/M3U8/PLS/XSPF files passed as a path or via stdin,
or a whole directory of them at once, can verify referenced files exist on
disk, and rewrites relative paths when a playlist moves to a different
directory.

## License

MIT, see [LICENSE](LICENSE).
