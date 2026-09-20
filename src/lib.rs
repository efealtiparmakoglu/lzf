//! lzf — a fast, zero-dependency LZ77 + canonical Huffman file compressor
//! with its own container format (LZF1).
//!
//! Crate layout:
//! - [`compress`] / [`decompress`]: the codec
//! - [`format`]: container framing constants
//! - [`huffman`], [`bitio`], [`tables`], [`matchfind`], [`crc32`]: building blocks
//! - [`cli`]: argument parsing, [`run`]: the CLI entry point

pub mod bitio;
pub mod cli;
pub mod compress;
pub mod crc32;
pub mod decompress;
pub mod error;
pub mod format;
pub mod huffman;
pub mod matchfind;
pub mod tables;

use std::fs;
use std::io::{self, BufReader, BufWriter, Write};
use std::path::{Path, PathBuf};
use std::process::ExitCode;
use std::time::{Duration, Instant};

use cli::{Cli, Command};
use error::{Error, Result};

/// CLI entry point (also exercised directly by the integration tests).
pub fn run(args: &[String]) -> ExitCode {
    let parsed = match cli::parse_args(args) {
        Ok(c) => c,
        Err(msg) => {
            eprintln!("lzf: {msg}");
            eprintln!("try 'lzf --help'");
            return ExitCode::from(2);
        }
    };
    execute(&parsed)
}

fn execute(cli: &Cli) -> ExitCode {
    let mut failed = false;
    match &cli.command {
        Command::Compress { files, output, level, force, rm } => {
            for f in files {
                if let Err(e) = compress_one(f, output.as_deref(), *level, *force, *rm, cli.quiet) {
                    eprintln!("lzf: {}: {e}", f.display());
                    failed = true;
                }
            }
        }
        Command::Decompress { files, output, force, rm } => {
            for f in files {
                if let Err(e) = decompress_one(f, output.as_deref(), *force, *rm, cli.quiet) {
                    eprintln!("lzf: {}: {e}", f.display());
                    failed = true;
                }
            }
        }
        Command::Test { files } => {
            for f in files {
                if let Err(e) = test_one(f) {
                    eprintln!("lzf: {}: {e}", f.display());
                    failed = true;
                }
            }
        }
    }
    if failed {
        ExitCode::FAILURE
    } else {
        ExitCode::SUCCESS
    }
}

fn compress_one(
    input: &Path,
    output: Option<&Path>,
    level: u8,
    force: bool,
    rm: bool,
    quiet: bool,
) -> Result<()> {
    if input.is_dir() {
        return Err(Error::Usage(format!("{} is a directory", input.display())));
    }
    let out_path = match output {
        Some(p) => p.to_path_buf(),
        None => PathBuf::from(format!("{}.lzf", input.display())),
    };
    if out_path.exists() && !force {
        return Err(Error::Exists(out_path.display().to_string()));
    }
    let name = input
        .file_name()
        .map(|n| n.to_string_lossy().into_owned())
        .unwrap_or_default();

    let f = fs::File::open(input)?;
    let out_file = fs::File::create(&out_path)?;
    let start = Instant::now();
    let result = compress::compress_stream(
        BufReader::with_capacity(1 << 20, f),
        BufWriter::with_capacity(1 << 20, out_file),
        level,
        name.as_bytes(),
    );
    match result {
        Ok((in_size, stats, _writer)) => {
            let dur = start.elapsed();
            if rm {
                let _ = fs::remove_file(input);
            }
            if !quiet {
                println!(
                    "{}: {} -> {} ({:.1}% saved) in {} [{:.1} MiB/s, level {}]",
                    out_path.display(),
                    human(in_size),
                    human(stats.output_size),
                    saved_pct(in_size, stats.output_size),
                    human_duration(dur),
                    mib_per_s(in_size, dur),
                    level,
                );
            }
            Ok(())
        }
        Err(e) => {
            // Don't leave a half-written container behind.
            let _ = fs::remove_file(&out_path);
            Err(e)
        }
    }
}

fn decompress_one(
    input: &Path,
    output: Option<&Path>,
    force: bool,
    rm: bool,
    quiet: bool,
) -> Result<()> {
    let out_path = match output {
        Some(p) => p.to_path_buf(),
        None => {
            let s = input.to_string_lossy();
            match s.strip_suffix(".lzf") {
                Some(base) if !base.is_empty() => PathBuf::from(base),
                _ => {
                    return Err(Error::Usage(
                        "unknown suffix for decompression (use -o)".into(),
                    ))
                }
            }
        }
    };
    if out_path.exists() && !force {
        return Err(Error::Exists(out_path.display().to_string()));
    }

    let f = fs::File::open(input)?;
    let out_file = fs::File::create(&out_path)?;
    let mut out_writer = BufWriter::with_capacity(1 << 20, out_file);
    let start = Instant::now();
    let result = decompress::decompress_stream(
        BufReader::with_capacity(1 << 20, f),
        &mut out_writer,
    );
    match result {
        Ok((header, written, _)) => {
            out_writer.flush()?;
            let dur = start.elapsed();
            if rm {
                let _ = fs::remove_file(input);
            }
            if !quiet {
                println!(
                    "{} -> {}: {} in {} [{:.1} MiB/s, crc32 ok, stored as \"{}\"]",
                    input.display(),
                    out_path.display(),
                    human(written),
                    human_duration(dur),
                    mib_per_s(written, dur),
                    header.name,
                );
            }
            Ok(())
        }
        Err(e) => {
            let _ = fs::remove_file(&out_path);
            Err(e)
        }
    }
}

fn test_one(input: &Path) -> Result<()> {
    let f = fs::File::open(input)?;
    let (header, written, _) = decompress::decompress_stream(
        BufReader::with_capacity(1 << 20, f),
        io::sink(),
    )?;
    println!(
        "{}: OK ({} bytes, crc32 ok, stored as \"{}\")",
        input.display(),
        written,
        header.name
    );
    Ok(())
}

fn saved_pct(original: u64, packed: u64) -> f64 {
    if original == 0 {
        return 0.0;
    }
    100.0 * (1.0 - packed as f64 / original as f64)
}

fn mib_per_s(bytes: u64, dur: Duration) -> f64 {
    let s = dur.as_secs_f64();
    if s <= 0.0 {
        return 0.0;
    }
    bytes as f64 / (1024.0 * 1024.0) / s
}

pub fn human(n: u64) -> String {
    const UNITS: [&str; 5] = ["B", "KiB", "MiB", "GiB", "TiB"];
    let mut v = n as f64;
    let mut u = 0usize;
    while v >= 1024.0 && u < UNITS.len() - 1 {
        v /= 1024.0;
        u += 1;
    }
    if u == 0 {
        format!("{n} B")
    } else {
        format!("{v:.1} {}", UNITS[u])
    }
}

fn human_duration(d: Duration) -> String {
    let ms = d.as_secs_f64() * 1000.0;
    if ms < 1.0 {
        format!("{:.2} ms", ms)
    } else if ms < 1000.0 {
        format!("{ms:.0} ms")
    } else {
        format!("{:.2} s", ms / 1000.0)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn human_sizes() {
        assert_eq!(human(0), "0 B");
        assert_eq!(human(512), "512 B");
        assert_eq!(human(2048), "2.0 KiB");
        assert_eq!(human(3 * 1024 * 1024), "3.0 MiB");
    }

    #[test]
    fn cli_parse_roundtrip() {
        let args: Vec<String> = ["c", "-9", "-f", "--rm", "-q", "a.txt", "b.txt"]
            .iter()
            .map(|s| s.to_string())
            .collect();
        let cli = cli::parse_args(&args).unwrap();
        match cli.command {
            Command::Compress { files, level, force, rm, .. } => {
                assert_eq!(files.len(), 2);
                assert_eq!(level, 9);
                assert!(force && rm);
            }
            _ => panic!("wrong command"),
        }
        assert!(cli.quiet);
    }

    #[test]
    fn cli_rejects_multi_file_output() {
        let args: Vec<String> = ["c", "-o", "x", "a", "b"].iter().map(|s| s.to_string()).collect();
        assert!(cli::parse_args(&args).is_err());
    }

    #[test]
    fn cli_rejects_bad_level() {
        let args: Vec<String> = ["c", "--level", "12", "a"].iter().map(|s| s.to_string()).collect();
        assert!(cli::parse_args(&args).is_err());
    }
}
