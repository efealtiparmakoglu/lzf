//! Run the actual CLI against disposable files; never use user data.
use std::fs;
use std::path::{Path, PathBuf};
use std::process::{Command, Output};
use std::sync::atomic::{AtomicU64, Ordering};

static NEXT_ID: AtomicU64 = AtomicU64::new(0);
struct Workspace(PathBuf);
impl Workspace {
    fn new() -> Self {
        let path = std::env::temp_dir().join(format!(
            "lzf-safe-{}-{}",
            std::process::id(),
            NEXT_ID.fetch_add(1, Ordering::Relaxed)
        ));
        fs::create_dir(&path).unwrap();
        Self(path)
    }
    fn path(&self, name: &str) -> PathBuf {
        self.0.join(name)
    }
    fn run(&self, args: &[&str]) -> Output {
        let result = Command::new(env!("CARGO_BIN_EXE_lzf"))
            .current_dir(&self.0)
            .args(args)
            .output()
            .unwrap();
        assert!(
            fs::read_dir(&self.0).unwrap().all(|p| !p
                .unwrap()
                .file_name()
                .to_string_lossy()
                .starts_with(".lzf-")),
            "temporary output leaked"
        );
        result
    }
}
impl Drop for Workspace {
    fn drop(&mut self) {
        let _ = fs::remove_dir_all(&self.0);
    }
}
fn content() -> Vec<u8> {
    b"important original content\n".repeat(1000)
}
fn packed(path: &Path) -> Vec<u8> {
    let bytes = lzf::compress::compress_bytes(&content(), 6, b"original").unwrap();
    fs::write(path, &bytes).unwrap();
    bytes
}

#[test]
fn same_input_and_output_is_rejected_even_with_force_and_rm() {
    for mode in ["c", "d"] {
        let w = Workspace::new();
        let expected = if mode == "d" {
            packed(&w.path("input"))
        } else {
            fs::write(w.path("input"), content()).unwrap();
            content()
        };
        let run = w.run(&[mode, "-f", "--rm", "-o", "./input", "input"]);
        assert_eq!(run.status.code(), Some(1));
        assert!(String::from_utf8_lossy(&run.stderr).contains("different files"));
        assert_eq!(fs::read(w.path("input")).unwrap(), expected);
    }
}

#[test]
fn corrupt_archive_preserves_existing_output_and_input() {
    let w = Workspace::new();
    let mut archive = packed(&w.path("input.lzf"));
    let last = archive.len() - 1;
    archive[last] ^= 1;
    fs::write(w.path("input.lzf"), &archive).unwrap();
    fs::write(w.path("output"), b"previous good output").unwrap();
    let run = w.run(&["d", "-f", "--rm", "-o", "output", "input.lzf"]);
    assert_eq!(run.status.code(), Some(1));
    assert!(String::from_utf8_lossy(&run.stderr).contains("checksum mismatch"));
    assert_eq!(fs::read(w.path("output")).unwrap(), b"previous good output");
    assert_eq!(fs::read(w.path("input.lzf")).unwrap(), archive);
}

#[test]
fn corrupt_archive_does_not_leave_a_partial_new_output() {
    let w = Workspace::new();
    fs::write(w.path("bad.lzf"), b"broken archive").unwrap();
    assert_eq!(
        w.run(&["d", "-o", "output", "bad.lzf"]).status.code(),
        Some(1)
    );
    assert!(!w.path("output").exists());
}

#[test]
fn existing_destination_requires_force_for_both_commands() {
    for mode in ["c", "d"] {
        let w = Workspace::new();
        packed(&w.path("input"));
        fs::write(w.path("output"), b"keep this").unwrap();
        assert_eq!(
            w.run(&[mode, "-o", "output", "input"]).status.code(),
            Some(1)
        );
        assert_eq!(fs::read(w.path("output")).unwrap(), b"keep this");
    }
}

#[test]
fn force_publishes_valid_output_before_removing_the_input() {
    let w = Workspace::new();
    fs::write(w.path("input"), content()).unwrap();
    fs::write(w.path("output.lzf"), b"replace me").unwrap();
    assert!(w
        .run(&["c", "-f", "--rm", "-o", "output.lzf", "input"])
        .status
        .success());
    assert!(!w.path("input").exists());
    assert_eq!(
        lzf::decompress::decompress_bytes(&fs::read(w.path("output.lzf")).unwrap()).unwrap(),
        content()
    );
    fs::write(w.path("restored"), b"replace me too").unwrap();
    assert!(w
        .run(&["d", "-f", "--rm", "-o", "restored", "output.lzf"])
        .status
        .success());
    assert_eq!(fs::read(w.path("restored")).unwrap(), content());
    assert!(!w.path("output.lzf").exists());
}

#[test]
fn failed_publish_preserves_input_and_destination_directory() {
    let w = Workspace::new();
    fs::write(w.path("input"), content()).unwrap();
    fs::create_dir(w.path("directory")).unwrap();
    fs::write(w.path("directory/keep"), b"keep").unwrap();
    assert_eq!(
        w.run(&["c", "-f", "--rm", "-o", "directory", "input"])
            .status
            .code(),
        Some(1)
    );
    assert_eq!(fs::read(w.path("input")).unwrap(), content());
    assert_eq!(fs::read(w.path("directory/keep")).unwrap(), b"keep");
}

#[cfg(unix)]
#[test]
fn aliases_of_the_input_are_rejected() {
    let w = Workspace::new();
    fs::write(w.path("input"), content()).unwrap();
    std::os::unix::fs::symlink("input", w.path("symbolic")).unwrap();
    fs::hard_link(w.path("input"), w.path("hard")).unwrap();
    for alias in ["symbolic", "hard"] {
        assert_eq!(
            w.run(&["c", "-f", "--rm", "-o", alias, "input"])
                .status
                .code(),
            Some(1)
        );
        assert_eq!(fs::read(w.path("input")).unwrap(), content());
        assert_eq!(fs::read(w.path(alias)).unwrap(), content());
    }
}
