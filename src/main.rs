use std::io::Read;
use std::path::{Path, PathBuf};
use std::process::ExitCode;

fn print_usage() {
    eprintln!("usage: playlist-tidy [--lenient] [--check] [-o OUTPUT] <INPUT|DIR|->");
    eprintln!();
    eprintln!("  INPUT        path to an .m3u/.m3u8/.pls/.xspf file, or - to read stdin");
    eprintln!("  DIR          a directory to scan recursively for playlist files");
    eprintln!("  -o, --output write the result here instead of stdout; a directory");
    eprintln!("               when INPUT is a directory (required in that case,");
    eprintln!("               unless --check is also given)");
    eprintln!("  --lenient    repair problems instead of rejecting the file");
    eprintln!("  --check      report whether the file(s) need repair; write nothing");
    eprintln!();
    eprintln!("Input format is picked from the file extension, or from the");
    eprintln!("content itself when reading stdin. Output is always M3U.");
}

fn is_playlist_extension(path: &Path) -> bool {
    matches!(
        path.extension()
            .and_then(|e| e.to_str())
            .map(|e| e.to_ascii_lowercase())
            .as_deref(),
        Some("m3u") | Some("m3u8") | Some("pls") | Some("xspf")
    )
}

/// Recursively finds playlist files under `dir`, returning their paths
/// relative to `root`. Directories are walked in sorted order so output is
/// deterministic across runs and filesystems.
fn collect_playlist_files(
    root: &Path,
    dir: &Path,
    out: &mut Vec<PathBuf>,
) -> std::io::Result<()> {
    let mut entries: Vec<_> = std::fs::read_dir(dir)?.collect::<Result<_, _>>()?;
    entries.sort_by_key(|e| e.path());
    for entry in entries {
        let path = entry.path();
        if path.is_dir() {
            collect_playlist_files(root, &path, out)?;
        } else if is_playlist_extension(&path) {
            if let Ok(relative) = path.strip_prefix(root) {
                out.push(relative.to_path_buf());
            }
        }
    }
    Ok(())
}

fn process_content(
    content: &str,
    display_path: &str,
    opts: &playlist_tidy::Options,
) -> Result<playlist_tidy::FormatResult, playlist_tidy::FormatError> {
    match playlist_tidy::detect_input_format(display_path, content) {
        playlist_tidy::InputFormat::M3u => playlist_tidy::format(content, opts),
        playlist_tidy::InputFormat::Pls => playlist_tidy::format_pls(content, opts),
        playlist_tidy::InputFormat::Xspf => playlist_tidy::format_xspf(content, opts),
    }
}

/// Processes every playlist file found under `dir`. Writes go under
/// `output_dir`, mirroring the input's directory structure with the
/// extension changed to `.m3u`, since output is always M3U regardless of
/// what format each file started as.
fn run_batch(
    dir: &str,
    opts: &playlist_tidy::Options,
    check: bool,
    output_dir: Option<&str>,
) -> ExitCode {
    let mut files = Vec::new();
    if let Err(e) = collect_playlist_files(Path::new(dir), Path::new(dir), &mut files) {
        eprintln!("error: failed to read directory '{}': {}", dir, e);
        return ExitCode::FAILURE;
    }

    if files.is_empty() {
        eprintln!("no .m3u/.m3u8/.pls/.xspf files found under '{}'", dir);
        return ExitCode::SUCCESS;
    }

    if !check && output_dir.is_none() {
        eprintln!("error: processing a directory requires -o/--output DIR");
        return ExitCode::from(2);
    }

    let mut clean = 0;
    let mut repaired = 0;
    let mut failed = 0;

    for relative in &files {
        let full_path = Path::new(dir).join(relative);
        let display = relative.to_string_lossy().to_string();

        let raw = match std::fs::read_to_string(&full_path) {
            Ok(s) => s,
            Err(e) => {
                eprintln!("error: {}: failed to read ({})", display, e);
                failed += 1;
                continue;
            }
        };

        let result = match process_content(&raw, &display, opts) {
            Ok(r) => r,
            Err(e) => {
                eprintln!("error: {}: {}", display, e);
                failed += 1;
                continue;
            }
        };

        for warning in &result.warnings {
            eprintln!("warning: {}: {}", display, warning);
        }

        if check {
            if result.warnings.is_empty() {
                clean += 1;
            } else {
                repaired += 1;
            }
            continue;
        }

        let out_dir = output_dir.expect("checked above");
        let out_path = Path::new(out_dir).join(relative.with_extension("m3u"));
        if let Some(parent) = out_path.parent() {
            if let Err(e) = std::fs::create_dir_all(parent) {
                eprintln!(
                    "error: {}: failed to create output directory ({})",
                    display, e
                );
                failed += 1;
                continue;
            }
        }
        match std::fs::write(&out_path, result.output) {
            Ok(()) => {
                if result.warnings.is_empty() {
                    clean += 1;
                } else {
                    repaired += 1;
                }
            }
            Err(e) => {
                eprintln!("error: {}: failed to write ({})", display, e);
                failed += 1;
            }
        }
    }

    eprintln!(
        "processed {} file{}: {} clean, {} repaired, {} failed",
        files.len(),
        if files.len() == 1 { "" } else { "s" },
        clean,
        repaired,
        failed
    );

    if failed > 0 {
        ExitCode::FAILURE
    } else if check && repaired > 0 {
        ExitCode::from(1)
    } else {
        ExitCode::SUCCESS
    }
}

fn main() -> ExitCode {
    let args: Vec<String> = std::env::args().collect();

    let mut lenient = false;
    let mut check = false;
    let mut input_path: Option<String> = None;
    let mut output_path: Option<String> = None;

    let mut i = 1;
    while i < args.len() {
        match args[i].as_str() {
            "--lenient" => lenient = true,
            "--check" => check = true,
            "-o" | "--output" => {
                i += 1;
                match args.get(i) {
                    Some(v) => output_path = Some(v.clone()),
                    None => {
                        eprintln!("error: {} requires a value", args[i - 1]);
                        return ExitCode::from(2);
                    }
                }
            }
            "-h" | "--help" => {
                print_usage();
                return ExitCode::SUCCESS;
            }
            other if input_path.is_none() => input_path = Some(other.to_string()),
            other => {
                eprintln!("error: unexpected argument '{}'", other);
                return ExitCode::from(2);
            }
        }
        i += 1;
    }

    let Some(input_path) = input_path else {
        print_usage();
        return ExitCode::from(2);
    };

    if check && output_path.is_some() {
        eprintln!("error: --check cannot be combined with -o/--output");
        return ExitCode::from(2);
    }

    if input_path != "-" && std::fs::metadata(&input_path).map(|m| m.is_dir()).unwrap_or(false) {
        let opts = playlist_tidy::Options { lenient };
        return run_batch(&input_path, &opts, check, output_path.as_deref());
    }

    let raw = if input_path == "-" {
        let mut buf = String::new();
        if let Err(e) = std::io::stdin().read_to_string(&mut buf) {
            eprintln!("error: failed to read stdin: {}", e);
            return ExitCode::FAILURE;
        }
        buf
    } else {
        match std::fs::read_to_string(&input_path) {
            Ok(s) => s,
            Err(e) => {
                eprintln!("error: failed to read '{}': {}", input_path, e);
                return ExitCode::FAILURE;
            }
        }
    };

    let opts = playlist_tidy::Options { lenient };
    let result = process_content(&raw, &input_path, &opts);
    match result {
        Ok(result) => {
            for warning in &result.warnings {
                eprintln!("warning: {}", warning);
            }
            if check {
                if result.warnings.is_empty() {
                    return ExitCode::SUCCESS;
                }
                eprintln!("playlist needs repair; rerun without --check to write the result");
                return ExitCode::from(1);
            }
            match output_path {
                Some(path) => {
                    if let Err(e) = std::fs::write(&path, result.output) {
                        eprintln!("error: failed to write '{}': {}", path, e);
                        return ExitCode::FAILURE;
                    }
                }
                None => print!("{}", result.output),
            }
            ExitCode::SUCCESS
        }
        Err(e) => {
            eprintln!("error: {}", e);
            if !lenient {
                eprintln!("hint: pass --lenient to repair the file instead of failing");
            }
            ExitCode::FAILURE
        }
    }
}
