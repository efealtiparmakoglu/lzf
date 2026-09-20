//! Hand-rolled argument parsing (zero dependencies) and usage text.

use std::path::PathBuf;

#[derive(Clone, Debug)]
pub enum Command {
    Compress {
        files: Vec<PathBuf>,
        output: Option<PathBuf>,
        level: u8,
        force: bool,
        rm: bool,
    },
    Decompress {
        files: Vec<PathBuf>,
        output: Option<PathBuf>,
        force: bool,
        rm: bool,
    },
    Test {
        files: Vec<PathBuf>,
    },
}

#[derive(Clone, Debug)]
pub struct Cli {
    pub command: Command,
    pub quiet: bool,
}

pub const USAGE: &str = "\
lzf 1.0.0 — fast LZ77+Huffman file compressor (LZF1 format)

USAGE:
    lzf compress [OPTIONS] <FILE>...    compress each FILE to FILE.lzf
    lzf decompress [OPTIONS] <FILE>...  decompress each FILE (aliases: d)
    lzf test <FILE>...                  verify integrity without writing

ALIASES:
    c, z  = compress        d, x  = decompress        t = test

OPTIONS:
    -o <FILE>     output path (single input only)
    -0 .. -9      compression level (default 6; 0 = store, 9 = slowest/best)
    --level <N>   same as -N
    -f, --force   overwrite existing output files
    --rm          remove input files after a successful run
    -q, --quiet   suppress progress output
    -h, --help    show this help
    -V, --version show version

EXAMPLES:
    lzf c notes.txt            # notes.txt -> notes.txt.lzf
    lzf c -9 backup.tar        # best effort
    lzf c *.log --rm           # compress and remove originals
    lzf d notes.txt.lzf        # -> notes.txt (crc32 verified)
    lzf t data.lzf             # integrity check only
";

#[derive(Default)]
struct Flags {
    output: Option<PathBuf>,
    level: u8,
    force: bool,
    rm: bool,
    quiet: bool,
}

/// Parses CLI arguments. `Err` carries a user-facing message.
pub fn parse_args(args: &[String]) -> Result<Cli, String> {
    if args.is_empty() {
        print!("{}", USAGE);
        std::process::exit(0);
    }
    let quiet_global = false;
    match args[0].as_str() {
        "help" | "--help" | "-h" => {
            print!("{}", USAGE);
            std::process::exit(0);
        }
        "--version" | "-V" | "version" => {
            println!("lzf 1.0.0");
            std::process::exit(0);
        }
        "c" | "z" | "compress" => {
            let (files, flags) = parse_opts(&args[1..], Mode::Compress)?;
            if files.is_empty() {
                return Err("compress: no input files".into());
            }
            if files.len() > 1 && flags.output.is_some() {
                return Err("compress: -o requires exactly one input file".into());
            }
            Ok(Cli {
                command: Command::Compress {
                    files,
                    output: flags.output,
                    level: flags.level,
                    force: flags.force,
                    rm: flags.rm,
                },
                quiet: flags.quiet || quiet_global,
            })
        }
        "d" | "x" | "decompress" => {
            let (files, flags) = parse_opts(&args[1..], Mode::Decompress)?;
            if files.is_empty() {
                return Err("decompress: no input files".into());
            }
            if files.len() > 1 && flags.output.is_some() {
                return Err("decompress: -o requires exactly one input file".into());
            }
            Ok(Cli {
                command: Command::Decompress {
                    files,
                    output: flags.output,
                    force: flags.force,
                    rm: flags.rm,
                },
                quiet: flags.quiet,
            })
        }
        "t" | "test" => {
            let (files, _) = parse_opts(&args[1..], Mode::Test)?;
            if files.is_empty() {
                return Err("test: no input files".into());
            }
            Ok(Cli { command: Command::Test { files }, quiet: false })
        }
        other => Err(format!("unknown command '{other}' (try --help)")),
    }
}

#[derive(Clone, Copy, PartialEq)]
enum Mode {
    Compress,
    Decompress,
    Test,
}

fn parse_opts(args: &[String], mode: Mode) -> Result<(Vec<PathBuf>, Flags), String> {
    let mut flags = Flags { level: 6, ..Flags::default() };
    let mut files = Vec::new();
    let mut i = 0;
    while i < args.len() {
        let a = &args[i];
        match a.as_str() {
            "-o" => {
                i += 1;
                let val = args.get(i).ok_or("-o requires a path")?;
                flags.output = Some(PathBuf::from(val));
            }
            "--level" => {
                i += 1;
                let val = args.get(i).ok_or("--level requires a number 0..=9")?;
                flags.level = parse_level(val)?;
            }
            "-f" | "--force" => flags.force = true,
            "--rm" => flags.rm = true,
            "-q" | "--quiet" => flags.quiet = true,
            "-h" | "--help" => {
                print!("{}", USAGE);
                std::process::exit(0);
            }
            "-V" | "--version" => {
                println!("lzf 1.0.0");
                std::process::exit(0);
            }
            _ if is_level_flag(a) => {
                flags.level = a[1..].parse::<u8>().map_err(|_| format!("bad level '{a}'"))?;
            }
            _ if a.starts_with('-') && a.len() > 1 => {
                return Err(format!("unknown option '{a}' (try --help)"));
            }
            _ => files.push(PathBuf::from(a)),
        }
        i += 1;
    }
    if mode == Mode::Test && (flags.output.is_some() || flags.rm) {
        return Err("test: -o and --rm are not supported".into());
    }
    Ok((files, flags))
}

fn is_level_flag(a: &str) -> bool {
    a.len() == 2 && a.starts_with('-') && a.as_bytes()[1].is_ascii_digit()
}

fn parse_level(v: &str) -> Result<u8, String> {
    let n: u8 = v.parse().map_err(|_| format!("bad level '{v}'"))?;
    if n > 9 {
        return Err(format!("level {n} out of range 0..=9"));
    }
    Ok(n)
}
