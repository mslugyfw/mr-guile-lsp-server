//! Build script: embed every `*.scm` under `deps/` into the binary so the
//! server is self-contained (no external Guile package install needed).
//!
//! Generates `OUT_DIR/deps_files.rs` containing:
//!   `pub static DEPS_FILES: &[(&str, &str)] = &[ (rel_path, include_str!(abs)), ... ];`
//! where the first element is the path relative to `deps/` and the second is
//! the file contents pulled in at compile time.

use std::fs;
use std::path::{Path, PathBuf};

fn main() {
    let deps_root = Path::new("deps");
    println!("cargo:rerun-if-changed=deps");
    println!("cargo:rerun-if-changed=build.rs");

    let mut entries: Vec<(String, PathBuf)> = Vec::new();
    if deps_root.exists() {
        collect(deps_root, deps_root, &mut entries);
    }
    entries.sort();

    let out_dir = std::env::var("OUT_DIR").expect("OUT_DIR");
    let out_path = PathBuf::from(&out_dir).join("deps_files.rs");

    let mut src = String::from("pub static DEPS_FILES: &[(&str, &str)] = &[\n");
    for (rel, abs) in &entries {
        let rel = rel.replace('\\', "/");
        // include_str! resolves relative to the generated file (in OUT_DIR),
        // so emit absolute paths to the real source files.
        let abs = fs::canonicalize(abs)
            .unwrap_or_else(|_| abs.clone())
            .to_string_lossy()
            .replace('\\', "/");
        src.push_str(&format!("    ({rel:?}, include_str!(\"{abs}\")),\n"));
    }
    src.push_str("];\n");

    fs::write(&out_path, src).expect("write deps_files.rs");
}

fn collect(root: &Path, dir: &Path, out: &mut Vec<(String, PathBuf)>) {
    let rd = match fs::read_dir(dir) {
        Ok(rd) => rd,
        Err(_) => return,
    };
    for entry in rd.flatten() {
        let path = entry.path();
        if path.is_dir() {
            collect(root, &path, out);
        } else if path.extension().and_then(|e| e.to_str()) == Some("scm") {
            let rel = path
                .strip_prefix(root)
                .expect("path under root")
                .to_string_lossy()
                .to_string();
            out.push((rel, path));
        }
    }
}
