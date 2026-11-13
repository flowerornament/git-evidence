//! Names the source this binary is built from: a SHA-256 over every `.rs`
//! file under `src/` (its path and bytes, in path order; editor and other
//! untracked files are not source), as the compile-time
//! variable `GIT_EVIDENCE_SOURCE`. The index records it beside the views and
//! derived tables it prepares, so a binary built from other source (a
//! candidate sharing the store) never answers with them (`src/index/`).

use sha2::Digest as _;
use std::fmt::Write as _;
use std::path::{Path, PathBuf};

fn files(dir: &Path, out: &mut Vec<PathBuf>) {
    let entries = std::fs::read_dir(dir).expect("the source directory is readable");
    for entry in entries {
        let path = entry.expect("a source entry is readable").path();
        if path.is_dir() {
            files(&path, out);
        } else if path.extension().is_some_and(|extension| extension == "rs") {
            out.push(path);
        }
    }
}

fn main() {
    let manifest = PathBuf::from(std::env::var_os("CARGO_MANIFEST_DIR").expect("cargo sets it"));
    let src = manifest.join("src");
    println!("cargo:rerun-if-changed={}", src.display());
    let mut paths = Vec::new();
    files(&src, &mut paths);
    paths.sort();
    let mut hasher = sha2::Sha256::new();
    for path in &paths {
        let name = path.strip_prefix(&manifest).expect("under the manifest");
        let bytes = std::fs::read(path).expect("a source file is readable");
        hasher.update(name.to_string_lossy().as_bytes());
        hasher.update([0]);
        hasher.update(u64::try_from(bytes.len()).expect("fits").to_le_bytes());
        hasher.update(&bytes);
    }
    let mut digest = String::from("sha256:");
    for byte in hasher.finalize() {
        write!(digest, "{byte:02x}").expect("writing to a String cannot fail");
    }
    println!("cargo:rustc-env=GIT_EVIDENCE_SOURCE={digest}");
}
