use std::io::Read;
use std::process::ExitCode;

fn print_usage() {
    eprintln!("usage: playlist-tidy [--lenient] [-o OUTPUT] <INPUT|->");
    eprintln!();
    eprintln!("  INPUT        path to an .m3u/.m3u8/.pls/.xspf file, or - to read stdin");
    eprintln!("  -o, --output write the result here instead of stdout");
    eprintln!("  --lenient    repair problems instead of rejecting the file");
    eprintln!();
    eprintln!("Input format is picked from the file extension, or from the");
    eprintln!("content itself when reading stdin. Output is always M3U.");
}

fn main() -> ExitCode {
    let args: Vec<String> = std::env::args().collect();

    let mut lenient = false;
    let mut input_path: Option<String> = None;
    let mut output_path: Option<String> = None;

    let mut i = 1;
    while i < args.len() {
        match args[i].as_str() {
            "--lenient" => lenient = true,
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
    let result = match playlist_tidy::detect_input_format(&input_path, &raw) {
        playlist_tidy::InputFormat::M3u => playlist_tidy::format(&raw, &opts),
        playlist_tidy::InputFormat::Pls => playlist_tidy::format_pls(&raw, &opts),
        playlist_tidy::InputFormat::Xspf => playlist_tidy::format_xspf(&raw, &opts),
    };
    match result {
        Ok(result) => {
            for warning in &result.warnings {
                eprintln!("warning: {}", warning);
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
