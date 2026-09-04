//! Core normalization logic for M3U playlists, kept separate from the CLI
//! so it can be unit tested without touching stdin/stdout or the filesystem.

use std::collections::BTreeMap;

/// Behavior switches for `format`. Strict is the default everywhere the
/// binary constructs this; only `--lenient` sets `lenient` to true.
pub struct Options {
    pub lenient: bool,
}

pub struct FormatResult {
    pub output: String,
    /// Non-empty only in lenient mode: each repair that was applied.
    pub warnings: Vec<String>,
}

#[derive(Debug)]
pub struct FormatError {
    pub message: String,
}

impl std::fmt::Display for FormatError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "{}", self.message)
    }
}

impl std::error::Error for FormatError {}

fn strip_bom(s: &str) -> &str {
    s.strip_prefix('\u{feff}').unwrap_or(s)
}

fn normalize_line_endings(s: &str) -> String {
    s.replace("\r\n", "\n").replace('\r', "\n")
}

/// Normalizes `input` into a strict M3U playlist: a leading `#EXTM3U` line,
/// one `#EXTINF:<duration>,<title>` line per entry, and one path/URI line
/// per entry, with consistent slashes and no stray blank lines.
///
/// In strict mode any deviation from that shape is a `FormatError`. In
/// lenient mode the same deviations are repaired and recorded as warnings.
pub fn format(input: &str, opts: &Options) -> Result<FormatResult, FormatError> {
    let mut warnings = Vec::new();
    let normalized = normalize_line_endings(strip_bom(input));

    let lines: Vec<String> = normalized
        .split('\n')
        .map(|l| l.trim_end().to_string())
        .filter(|l| !l.is_empty())
        .collect();

    let mut idx = 0;
    let mut out = String::new();

    if lines.first().map(|s| s.as_str()) == Some("#EXTM3U") {
        idx = 1;
    } else if opts.lenient {
        warnings.push("missing #EXTM3U header, inserted one".to_string());
    } else {
        return Err(FormatError {
            message: "playlist is missing the #EXTM3U header".to_string(),
        });
    }
    out.push_str("#EXTM3U\n");

    let mut pending_extinf: Option<(i64, String)> = None;

    while idx < lines.len() {
        let line_no = idx + 1;
        let line = lines[idx].clone();
        idx += 1;

        if let Some(rest) = line.strip_prefix("#EXTINF:") {
            if pending_extinf.is_some() {
                let msg = format!("#EXTINF at line {} was never followed by an entry", line_no);
                if opts.lenient {
                    warnings.push(msg);
                    pending_extinf = None;
                } else {
                    return Err(FormatError { message: msg });
                }
            }
            match parse_extinf(rest) {
                Ok(parsed) => pending_extinf = Some(parsed),
                Err(reason) => {
                    if opts.lenient {
                        warnings.push(format!(
                            "repaired malformed #EXTINF at line {} ({})",
                            line_no, reason
                        ));
                        pending_extinf = Some(repair_extinf(rest));
                    } else {
                        return Err(FormatError {
                            message: format!("malformed #EXTINF at line {}: {}", line_no, reason),
                        });
                    }
                }
            }
            continue;
        }

        if line.starts_with('#') {
            // Unrecognized directive (e.g. #EXTGRP, #PLAYLIST): pass through
            // untouched rather than guessing what it means.
            out.push_str(&line);
            out.push('\n');
            continue;
        }

        let path = match normalize_path(&line) {
            Ok(p) => p,
            Err(reason) => {
                if opts.lenient {
                    warnings.push(format!("repaired path at line {} ({})", line_no, reason));
                    line.replace('\\', "/")
                } else {
                    return Err(FormatError {
                        message: format!("invalid path at line {}: {}", line_no, reason),
                    });
                }
            }
        };

        match pending_extinf.take() {
            Some((duration, title)) => {
                out.push_str(&format!("#EXTINF:{},{}\n", duration, title));
            }
            None => {
                if opts.lenient {
                    let title = derive_title(&path);
                    warnings.push(format!(
                        "entry at line {} had no #EXTINF, synthesized one",
                        line_no
                    ));
                    out.push_str(&format!("#EXTINF:-1,{}\n", title));
                } else {
                    return Err(FormatError {
                        message: format!("entry at line {} is missing a preceding #EXTINF", line_no),
                    });
                }
            }
        }
        out.push_str(&path);
        out.push('\n');
    }

    if pending_extinf.is_some() {
        let msg = "playlist ends with a dangling #EXTINF and no entry".to_string();
        if opts.lenient {
            warnings.push(msg);
        } else {
            return Err(FormatError { message: msg });
        }
    }

    Ok(FormatResult { output: out, warnings })
}

/// Parses the part of an `#EXTINF:` line after the colon. Duration must be
/// an integer, `-1` meaning "unknown/live", negative values below that are
/// rejected rather than silently clamped.
fn parse_extinf(rest: &str) -> Result<(i64, String), String> {
    let comma = rest
        .find(',')
        .ok_or_else(|| "missing comma between duration and title".to_string())?;
    let dur_str = rest[..comma].trim();
    let title = rest[comma + 1..].trim();

    let duration: i64 = dur_str
        .parse()
        .map_err(|_| format!("duration '{}' is not an integer", dur_str))?;
    if duration < -1 {
        return Err(format!("duration {} is negative", duration));
    }
    if title.is_empty() {
        return Err("title is empty".to_string());
    }
    Ok((duration, title.to_string()))
}

/// Best-effort recovery for a malformed `#EXTINF:` body, used only in
/// lenient mode. Never fails: falls back to -1 duration and "Unknown" title.
fn repair_extinf(rest: &str) -> (i64, String) {
    match rest.find(',') {
        Some(comma) => {
            let duration = rest[..comma].trim().parse::<i64>().unwrap_or(-1).max(-1);
            let title = rest[comma + 1..].trim();
            let title = if title.is_empty() { "Unknown".to_string() } else { title.to_string() };
            (duration, title)
        }
        None => {
            let title = rest.trim();
            let title = if title.is_empty() { "Unknown".to_string() } else { title.to_string() };
            (-1, title)
        }
    }
}

fn is_url(line: &str) -> bool {
    line.contains("://")
}

fn normalize_path(line: &str) -> Result<String, String> {
    if is_url(line) {
        return Ok(line.to_string());
    }
    if line.contains('\\') {
        return Err("path uses backslashes instead of forward slashes".to_string());
    }
    Ok(line.to_string())
}

fn derive_title(path: &str) -> String {
    path.rsplit('/').next().unwrap_or(path).to_string()
}

/// Which of the supported playlist formats an input file is written in.
#[derive(Debug, PartialEq, Eq)]
pub enum InputFormat {
    M3u,
    Pls,
    Xspf,
}

/// Picks a format from the file extension, falling back to sniffing the
/// first non-blank line for stdin or extensionless paths. PLS's only
/// reliable signature is the `[playlist]` section header, XSPF's is an XML
/// declaration or a `<playlist` root tag; everything else is assumed to be
/// M3U, since that's already the tolerant default.
pub fn detect_input_format(path: &str, content: &str) -> InputFormat {
    let lower = path.to_ascii_lowercase();
    if lower.ends_with(".pls") {
        return InputFormat::Pls;
    }
    if lower.ends_with(".xspf") {
        return InputFormat::Xspf;
    }
    if lower.ends_with(".m3u") || lower.ends_with(".m3u8") {
        return InputFormat::M3u;
    }
    let sniffed = normalize_line_endings(strip_bom(content));
    match sniffed.lines().map(|l| l.trim()).find(|l| !l.is_empty()) {
        Some(first) if first.eq_ignore_ascii_case("[playlist]") => InputFormat::Pls,
        Some(first)
            if first.starts_with("<?xml") || first.to_ascii_lowercase().starts_with("<playlist") =>
        {
            InputFormat::Xspf
        }
        _ => InputFormat::M3u,
    }
}

#[derive(Default)]
struct PlsEntry {
    file: Option<String>,
    title: Option<String>,
    length: Option<i64>,
}

/// Normalizes a PLS playlist into the same strict M3U output `format`
/// produces, so downstream consumers never need to care which format the
/// input arrived in.
pub fn format_pls(input: &str, opts: &Options) -> Result<FormatResult, FormatError> {
    let mut warnings = Vec::new();
    let normalized = normalize_line_endings(strip_bom(input));
    let lines: Vec<String> = normalized.lines().map(|l| l.trim().to_string()).collect();

    let mut idx = 0;
    while idx < lines.len() && lines[idx].is_empty() {
        idx += 1;
    }

    if lines.get(idx).map(|s| s.eq_ignore_ascii_case("[playlist]")) == Some(true) {
        idx += 1;
    } else if opts.lenient {
        warnings.push("missing [playlist] header, assumed one".to_string());
    } else {
        return Err(FormatError {
            message: "PLS file is missing the [playlist] header".to_string(),
        });
    }

    let mut entries: BTreeMap<u32, PlsEntry> = BTreeMap::new();
    let mut declared_count: Option<u32> = None;

    while idx < lines.len() {
        let line_no = idx + 1;
        let line = lines[idx].clone();
        idx += 1;
        if line.is_empty() {
            continue;
        }

        let Some(eq) = line.find('=') else {
            let msg = format!("line {} is not a key=value pair: '{}'", line_no, line);
            if opts.lenient {
                warnings.push(msg);
                continue;
            } else {
                return Err(FormatError { message: msg });
            }
        };
        let key = line[..eq].trim();
        let value = line[eq + 1..].trim();
        let lower_key = key.to_ascii_lowercase();

        if let Some(n) = lower_key.strip_prefix("file").and_then(|s| s.parse::<u32>().ok()) {
            entries.entry(n).or_default().file = Some(value.to_string());
        } else if let Some(n) = lower_key.strip_prefix("title").and_then(|s| s.parse::<u32>().ok()) {
            entries.entry(n).or_default().title = Some(value.to_string());
        } else if let Some(n) = lower_key.strip_prefix("length").and_then(|s| s.parse::<u32>().ok()) {
            match value.parse::<i64>() {
                Ok(v) => entries.entry(n).or_default().length = Some(v),
                Err(_) => {
                    let msg = format!("Length{} at line {} is not an integer: '{}'", n, line_no, value);
                    if opts.lenient {
                        warnings.push(msg);
                        entries.entry(n).or_default().length = Some(-1);
                    } else {
                        return Err(FormatError { message: msg });
                    }
                }
            }
        } else if lower_key == "numberofentries" {
            match value.parse::<u32>() {
                Ok(v) => declared_count = Some(v),
                Err(_) => {
                    let msg = format!("NumberOfEntries at line {} is not an integer: '{}'", line_no, value);
                    if opts.lenient {
                        warnings.push(msg);
                    } else {
                        return Err(FormatError { message: msg });
                    }
                }
            }
        } else if lower_key == "version" {
            // Not needed to build the output; every PLS version in the wild
            // uses the same File/Title/Length scheme.
        } else {
            let msg = format!("unrecognized key at line {}: '{}'", line_no, key);
            if opts.lenient {
                warnings.push(msg);
            } else {
                return Err(FormatError { message: msg });
            }
        }
    }

    match declared_count {
        Some(declared) if declared as usize != entries.len() => {
            let msg = format!(
                "NumberOfEntries says {} but {} entries were found",
                declared,
                entries.len()
            );
            if opts.lenient {
                warnings.push(msg);
            } else {
                return Err(FormatError { message: msg });
            }
        }
        Some(_) => {}
        None => {
            let msg = "PLS file has no NumberOfEntries".to_string();
            if opts.lenient {
                warnings.push("missing NumberOfEntries, inferred from entries found".to_string());
            } else {
                return Err(FormatError { message: msg });
            }
        }
    }

    if entries.is_empty() {
        return Err(FormatError {
            message: "PLS file has no entries".to_string(),
        });
    }

    let mut out = String::from("#EXTM3U\n");
    for (n, entry) in entries {
        let Some(file) = entry.file else {
            let msg = format!("entry {} has no File{} key", n, n);
            if opts.lenient {
                warnings.push(msg);
                continue;
            } else {
                return Err(FormatError { message: msg });
            }
        };

        let path = match normalize_path(&file) {
            Ok(p) => p,
            Err(reason) => {
                if opts.lenient {
                    warnings.push(format!("repaired path for entry {} ({})", n, reason));
                    file.replace('\\', "/")
                } else {
                    return Err(FormatError {
                        message: format!("invalid path for entry {}: {}", n, reason),
                    });
                }
            }
        };

        let title = match entry.title {
            Some(t) if !t.trim().is_empty() => t,
            _ => {
                let msg = format!("entry {} has no Title{} key", n, n);
                if opts.lenient {
                    warnings.push(msg);
                    derive_title(&path)
                } else {
                    return Err(FormatError { message: msg });
                }
            }
        };

        let duration = match entry.length {
            Some(d) if d >= -1 => d,
            Some(d) => {
                let msg = format!("entry {} has a negative Length{} ({})", n, n, d);
                if opts.lenient {
                    warnings.push(msg);
                    -1
                } else {
                    return Err(FormatError { message: msg });
                }
            }
            None => {
                let msg = format!("entry {} has no Length{} key", n, n);
                if opts.lenient {
                    warnings.push(msg);
                    -1
                } else {
                    return Err(FormatError { message: msg });
                }
            }
        };

        out.push_str(&format!("#EXTINF:{},{}\n", duration, title));
        out.push_str(&path);
        out.push('\n');
    }

    Ok(FormatResult { output: out, warnings })
}

/// Normalizes an XSPF (XML Shareable Playlist Format) playlist into the same
/// strict M3U output `format` produces. XSPF is XML, but pulling in a real
/// XML parser would break the no-dependencies rule, so this scans by hand
/// for the handful of elements a playlist actually uses.
pub fn format_xspf(input: &str, opts: &Options) -> Result<FormatResult, FormatError> {
    let mut warnings = Vec::new();
    let document = strip_xml_comments(strip_bom(input));

    let track_list_body;
    match find_element_body(&document, "trackList") {
        Some(body) => track_list_body = body,
        None => {
            let msg = "XSPF file is missing a <trackList> element".to_string();
            if opts.lenient {
                warnings.push(
                    "missing <trackList> element, scanning the whole document for tracks"
                        .to_string(),
                );
                track_list_body = document.as_str();
            } else {
                return Err(FormatError { message: msg });
            }
        }
    }

    let tracks = find_element_bodies(track_list_body, "track");
    if tracks.is_empty() {
        return Err(FormatError {
            message: "XSPF file has no <track> entries".to_string(),
        });
    }

    let mut out = String::from("#EXTM3U\n");
    for (i, track_body) in tracks.iter().enumerate() {
        let n = i + 1;

        let location = match find_element_body(track_body, "location") {
            Some(loc) if !loc.trim().is_empty() => decode_entities(loc.trim()),
            _ => {
                let msg = format!("track {} has no <location> element", n);
                if opts.lenient {
                    warnings.push(msg);
                    continue;
                } else {
                    return Err(FormatError { message: msg });
                }
            }
        };
        let raw_path = strip_file_uri(&location);

        let path = match normalize_path(&raw_path) {
            Ok(p) => p,
            Err(reason) => {
                if opts.lenient {
                    warnings.push(format!("repaired path for track {} ({})", n, reason));
                    raw_path.replace('\\', "/")
                } else {
                    return Err(FormatError {
                        message: format!("invalid path for track {}: {}", n, reason),
                    });
                }
            }
        };

        let title = match find_element_body(track_body, "title") {
            Some(t) if !t.trim().is_empty() => decode_entities(t.trim()),
            _ => derive_title(&path),
        };

        let duration = match find_element_body(track_body, "duration") {
            Some(d) if !d.trim().is_empty() => match d.trim().parse::<i64>() {
                Ok(ms) if ms >= 0 => ms / 1000,
                _ => {
                    let msg = format!(
                        "track {} has a non-numeric or negative <duration> ({})",
                        n,
                        d.trim()
                    );
                    if opts.lenient {
                        warnings.push(msg);
                        -1
                    } else {
                        return Err(FormatError { message: msg });
                    }
                }
            },
            // XSPF's <duration> is optional, unlike PLS's LengthN; -1 (unknown) is a
            // faithful reading of the spec, not a repair, so it needs no warning.
            _ => -1,
        };

        out.push_str(&format!("#EXTINF:{},{}\n", duration, title));
        out.push_str(&path);
        out.push('\n');
    }

    Ok(FormatResult { output: out, warnings })
}

fn strip_xml_comments(s: &str) -> String {
    let mut out = String::with_capacity(s.len());
    let mut rest = s;
    while let Some(start) = rest.find("<!--") {
        out.push_str(&rest[..start]);
        match rest[start..].find("-->") {
            Some(end) => rest = &rest[start + end + 3..],
            None => return out, // unterminated comment: drop the remainder
        }
    }
    out.push_str(rest);
    out
}

fn tag_boundary_ok(doc: &str, pos: usize) -> bool {
    match doc.as_bytes().get(pos) {
        None | Some(b' ') | Some(b'\t') | Some(b'\n') | Some(b'\r') | Some(b'>') | Some(b'/') => {
            true
        }
        _ => false,
    }
}

/// Returns the inner text of every top-level `<tag>...</tag>` (or
/// self-closing `<tag/>`, which yields an empty body) found in `doc`.
/// Good enough for XSPF's shallow, non-recursive `track` elements; not a
/// general XML parser.
fn find_element_bodies<'a>(doc: &'a str, tag: &str) -> Vec<&'a str> {
    let mut out = Vec::new();
    let open_needle = format!("<{}", tag);
    let close_needle = format!("</{}>", tag);
    let mut pos = 0;

    while let Some(rel) = doc[pos..].find(&open_needle) {
        let open_start = pos + rel;
        let after_needle = open_start + open_needle.len();
        if !tag_boundary_ok(doc, after_needle) {
            pos = after_needle;
            continue;
        }

        let Some(gt_rel) = doc[after_needle..].find('>') else {
            break; // unterminated opening tag
        };
        let gt = after_needle + gt_rel;

        if gt > 0 && doc.as_bytes()[gt - 1] == b'/' {
            out.push(&doc[gt..gt]);
            pos = gt + 1;
            continue;
        }

        let body_start = gt + 1;
        match doc[body_start..].find(&close_needle) {
            Some(close_rel) => {
                let body_end = body_start + close_rel;
                out.push(&doc[body_start..body_end]);
                pos = body_end + close_needle.len();
            }
            None => break, // unterminated element
        }
    }

    out
}

fn find_element_body<'a>(doc: &'a str, tag: &str) -> Option<&'a str> {
    find_element_bodies(doc, tag).into_iter().next()
}

/// Decodes the five predefined XML entities plus numeric character
/// references (`&#65;`, `&#x41;`). Unrecognized entities are left as-is.
fn decode_entities(s: &str) -> String {
    let mut out = String::with_capacity(s.len());
    let bytes = s.as_bytes();
    let mut i = 0;
    while i < s.len() {
        if bytes[i] == b'&' {
            if let Some(semi_rel) = s[i..].find(';') {
                let semi = i + semi_rel;
                let entity = &s[i + 1..semi];
                let replacement = match entity {
                    "amp" => Some('&'),
                    "lt" => Some('<'),
                    "gt" => Some('>'),
                    "quot" => Some('"'),
                    "apos" => Some('\''),
                    _ if entity.starts_with("#x") || entity.starts_with("#X") => {
                        u32::from_str_radix(&entity[2..], 16).ok().and_then(char::from_u32)
                    }
                    _ if entity.starts_with('#') => {
                        entity[1..].parse::<u32>().ok().and_then(char::from_u32)
                    }
                    _ => None,
                };
                if let Some(c) = replacement {
                    out.push(c);
                    i = semi + 1;
                    continue;
                }
            }
        }
        let ch = s[i..].chars().next().unwrap();
        out.push(ch);
        i += ch.len_utf8();
    }
    out
}

fn percent_decode(s: &str) -> String {
    let bytes = s.as_bytes();
    let mut out = Vec::with_capacity(bytes.len());
    let mut i = 0;
    while i < bytes.len() {
        if bytes[i] == b'%' && i + 2 < bytes.len() {
            if let Ok(byte) = u8::from_str_radix(&s[i + 1..i + 3], 16) {
                out.push(byte);
                i += 3;
                continue;
            }
        }
        out.push(bytes[i]);
        i += 1;
    }
    String::from_utf8(out).unwrap_or_else(|_| s.to_string())
}

/// Strips a `file://` prefix and percent-decodes what's left, since XSPF
/// requires `location` to be a URI even for plain local paths. Other schemes
/// (`http://`, etc.) are left untouched for `normalize_path` to pass through.
fn strip_file_uri(location: &str) -> String {
    match location.strip_prefix("file://") {
        Some(rest) => percent_decode(rest),
        None => location.to_string(),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn strict() -> Options {
        Options { lenient: false }
    }

    fn lenient() -> Options {
        Options { lenient: true }
    }

    #[test]
    fn passes_through_a_clean_playlist() {
        let input = "#EXTM3U\n#EXTINF:123,Artist - Title\nsongs/track.mp3\n";
        let result = format(input, &strict()).unwrap();
        assert_eq!(result.output, input);
        assert!(result.warnings.is_empty());
    }

    #[test]
    fn strict_rejects_missing_header() {
        let input = "#EXTINF:123,Artist - Title\nsongs/track.mp3\n";
        assert!(format(input, &strict()).is_err());
    }

    #[test]
    fn lenient_inserts_missing_header() {
        let input = "#EXTINF:123,Artist - Title\nsongs/track.mp3\n";
        let result = format(input, &lenient()).unwrap();
        assert!(result.output.starts_with("#EXTM3U\n"));
        assert!(!result.warnings.is_empty());
    }

    #[test]
    fn strict_rejects_backslash_paths() {
        let input = "#EXTM3U\n#EXTINF:123,Title\nC:\\Music\\track.mp3\n";
        assert!(format(input, &strict()).is_err());
    }

    #[test]
    fn lenient_converts_backslash_paths() {
        let input = "#EXTM3U\n#EXTINF:123,Title\nC:\\Music\\track.mp3\n";
        let result = format(input, &lenient()).unwrap();
        assert!(result.output.contains("C:/Music/track.mp3"));
    }

    #[test]
    fn drops_blank_lines_and_crlf() {
        let input = "#EXTM3U\r\n\r\n#EXTINF:5,Title\r\n\r\ntrack.mp3\r\n";
        let result = format(input, &strict()).unwrap();
        assert_eq!(result.output, "#EXTM3U\n#EXTINF:5,Title\ntrack.mp3\n");
    }

    #[test]
    fn strict_rejects_entry_without_extinf() {
        let input = "#EXTM3U\ntrack.mp3\n";
        assert!(format(input, &strict()).is_err());
    }

    #[test]
    fn lenient_synthesizes_missing_extinf() {
        let input = "#EXTM3U\nsongs/track.mp3\n";
        let result = format(input, &lenient()).unwrap();
        assert!(result.output.contains("#EXTINF:-1,track.mp3"));
    }

    #[test]
    fn detects_format_from_extension() {
        assert_eq!(detect_input_format("mix.pls", ""), InputFormat::Pls);
        assert_eq!(detect_input_format("mix.m3u8", ""), InputFormat::M3u);
    }

    #[test]
    fn detects_pls_from_content_when_extension_is_ambiguous() {
        let content = "[playlist]\nFile1=track.mp3\n";
        assert_eq!(detect_input_format("-", content), InputFormat::Pls);
        assert_eq!(detect_input_format("-", "#EXTM3U\n"), InputFormat::M3u);
    }

    #[test]
    fn pls_passes_through_a_clean_playlist() {
        let input = "[playlist]\nFile1=songs/track.mp3\nTitle1=Artist - Title\nLength1=123\nNumberOfEntries=1\nVersion=2\n";
        let result = format_pls(input, &strict()).unwrap();
        assert_eq!(result.output, "#EXTM3U\n#EXTINF:123,Artist - Title\nsongs/track.mp3\n");
        assert!(result.warnings.is_empty());
    }

    #[test]
    fn pls_strict_rejects_missing_header() {
        let input = "File1=track.mp3\nTitle1=Title\nLength1=1\nNumberOfEntries=1\n";
        assert!(format_pls(input, &strict()).is_err());
    }

    #[test]
    fn pls_lenient_assumes_missing_header() {
        let input = "File1=track.mp3\nTitle1=Title\nLength1=1\nNumberOfEntries=1\n";
        let result = format_pls(input, &lenient()).unwrap();
        assert!(result.output.contains("track.mp3"));
        assert!(!result.warnings.is_empty());
    }

    #[test]
    fn pls_strict_rejects_entry_count_mismatch() {
        let input = "[playlist]\nFile1=a.mp3\nTitle1=A\nLength1=1\nNumberOfEntries=2\n";
        assert!(format_pls(input, &strict()).is_err());
    }

    #[test]
    fn pls_lenient_converts_backslash_paths() {
        let input = "[playlist]\nFile1=C:\\Music\\track.mp3\nTitle1=Title\nLength1=1\nNumberOfEntries=1\n";
        let result = format_pls(input, &lenient()).unwrap();
        assert!(result.output.contains("C:/Music/track.mp3"));
    }

    #[test]
    fn pls_lenient_fills_in_missing_title_and_length() {
        let input = "[playlist]\nFile1=songs/track.mp3\nNumberOfEntries=1\n";
        let result = format_pls(input, &lenient()).unwrap();
        assert!(result.output.contains("#EXTINF:-1,track.mp3\nsongs/track.mp3"));
    }

    #[test]
    fn pls_strict_rejects_missing_title() {
        let input = "[playlist]\nFile1=songs/track.mp3\nLength1=10\nNumberOfEntries=1\n";
        assert!(format_pls(input, &strict()).is_err());
    }

    #[test]
    fn pls_orders_entries_by_index_not_by_appearance() {
        let input = "[playlist]\nFile2=b.mp3\nTitle2=B\nLength2=2\nFile1=a.mp3\nTitle1=A\nLength1=1\nNumberOfEntries=2\n";
        let result = format_pls(input, &strict()).unwrap();
        let a_pos = result.output.find("a.mp3").unwrap();
        let b_pos = result.output.find("b.mp3").unwrap();
        assert!(a_pos < b_pos);
    }

    #[test]
    fn xspf_passes_through_a_clean_playlist() {
        let input = "<?xml version=\"1.0\" encoding=\"UTF-8\"?>\n\
            <playlist version=\"1\" xmlns=\"http://xspf.org/ns/0/\">\n\
            <trackList>\n\
            <track><location>songs/track.mp3</location><title>Artist - Title</title>\
            <duration>123000</duration></track>\n\
            </trackList>\n\
            </playlist>\n";
        let result = format_xspf(input, &strict()).unwrap();
        assert_eq!(
            result.output,
            "#EXTM3U\n#EXTINF:123,Artist - Title\nsongs/track.mp3\n"
        );
        assert!(result.warnings.is_empty());
    }

    #[test]
    fn xspf_strict_rejects_missing_tracklist() {
        let input = "<?xml version=\"1.0\"?><playlist><track><location>a.mp3</location></track></playlist>";
        assert!(format_xspf(input, &strict()).is_err());
    }

    #[test]
    fn xspf_lenient_scans_document_without_tracklist() {
        let input = "<?xml version=\"1.0\"?><playlist><track><location>a.mp3</location></track></playlist>";
        let result = format_xspf(input, &lenient()).unwrap();
        assert!(result.output.contains("a.mp3"));
        assert!(!result.warnings.is_empty());
    }

    #[test]
    fn xspf_strict_rejects_missing_location() {
        let input = "<playlist><trackList><track><title>No file</title></track></trackList></playlist>";
        assert!(format_xspf(input, &strict()).is_err());
    }

    #[test]
    fn xspf_lenient_skips_track_without_location() {
        let input = "<playlist><trackList>\
            <track><title>No file</title></track>\
            <track><location>b.mp3</location></track>\
            </trackList></playlist>";
        let result = format_xspf(input, &lenient()).unwrap();
        assert!(!result.output.contains("No file"));
        assert!(result.output.contains("b.mp3"));
    }

    #[test]
    fn xspf_missing_duration_defaults_to_unknown_without_warning() {
        let input = "<playlist><trackList><track><location>a.mp3</location></track></trackList></playlist>";
        let result = format_xspf(input, &strict()).unwrap();
        assert!(result.output.contains("#EXTINF:-1,a.mp3"));
        assert!(result.warnings.is_empty());
    }

    #[test]
    fn xspf_converts_milliseconds_to_seconds() {
        let input = "<playlist><trackList><track><location>a.mp3</location><duration>65000</duration></track></trackList></playlist>";
        let result = format_xspf(input, &strict()).unwrap();
        assert!(result.output.contains("#EXTINF:65,"));
    }

    #[test]
    fn xspf_strict_rejects_non_numeric_duration() {
        let input = "<playlist><trackList><track><location>a.mp3</location><duration>long</duration></track></trackList></playlist>";
        assert!(format_xspf(input, &strict()).is_err());
    }

    #[test]
    fn xspf_decodes_entities_and_strips_file_uri() {
        let input = "<playlist><trackList><track>\
            <location>file:///music/AC%26DC/track.mp3</location>\
            <title>Rock &amp; Roll</title>\
            </track></trackList></playlist>";
        let result = format_xspf(input, &strict()).unwrap();
        assert!(result.output.contains("Rock & Roll"));
        assert!(result.output.contains("/music/AC&DC/track.mp3"));
    }

    #[test]
    fn xspf_lenient_converts_backslash_paths() {
        let input = "<playlist><trackList><track><location>C:\\Music\\track.mp3</location></track></trackList></playlist>";
        let result = format_xspf(input, &lenient()).unwrap();
        assert!(result.output.contains("C:/Music/track.mp3"));
    }

    #[test]
    fn xspf_derives_title_when_missing() {
        let input = "<playlist><trackList><track><location>songs/track.mp3</location></track></trackList></playlist>";
        let result = format_xspf(input, &strict()).unwrap();
        assert!(result.output.contains("#EXTINF:-1,track.mp3"));
    }

    #[test]
    fn detects_xspf_from_extension_and_content() {
        assert_eq!(detect_input_format("mix.xspf", ""), InputFormat::Xspf);
        assert_eq!(
            detect_input_format("-", "<?xml version=\"1.0\"?>\n<playlist></playlist>"),
            InputFormat::Xspf
        );
        assert_eq!(
            detect_input_format("-", "<playlist xmlns=\"http://xspf.org/ns/0/\">"),
            InputFormat::Xspf
        );
    }
}
