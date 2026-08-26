//! Core normalization logic for M3U playlists, kept separate from the CLI
//! so it can be unit tested without touching stdin/stdout or the filesystem.

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
}
