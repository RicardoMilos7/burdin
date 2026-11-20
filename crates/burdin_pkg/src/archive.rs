//! `burdin package`: the file selection and the archive of one package (`spec/packages.md`, PKG20-PKG22).
//!
//! The archive is a USTAR tar with no compression and nothing that varies between hosts or runs: entries sorted by
//! path, regular files only, mode `0644`, owner `0`, modification time `0`. The same tree therefore packs to the same
//! bytes everywhere, which is what the printed `sha256:` checksum lets a reader check.

use burdin_query::package::{Manifest, parse_manifest, source_path_text};
use std::path::{Path, PathBuf};

/// The top-level directories a package without `include` leaves out: git's metadata, build output, and the copies
/// `burdin vendor` writes (PKG21).
const DEFAULT_EXCLUDED: [&str; 3] = [".git", "target", "vendor"];

/// The directory `burdin package` writes archives into, relative to the package (`target` is build output).
const OUTPUT_DIR: &str = "target/package";

/// What `package` wrote.
#[derive(Debug)]
pub struct Packaged {
    /// The archive file.
    pub path: PathBuf,
    /// How many files it holds.
    pub files: usize,
    /// `sha256:` and the hexadecimal SHA-256 of the archive's bytes.
    pub checksum: String,
}

/// Writes the archive of the package in `dir` to `target/package/<name>-<version>.tar` (PKG20). A package without a
/// `version` is an error: the archive is named by it.
pub fn package(dir: &Path) -> Result<Packaged, String> {
    let manifest_path = dir.join("burdin.toml");
    let manifest = parse_manifest(&manifest_path)?;
    let Some(version) = manifest.version.clone() else {
        return Err(format!(
            "{}: `burdin package` needs a `version` in [package]: the archive is named by it",
            source_path_text(&manifest_path)
        ));
    };
    let files = selected_files(dir, &manifest)?;
    let mut entries = Vec::with_capacity(files.len());
    for (relative, path) in &files {
        let data = std::fs::read(path).map_err(|e| format!("cannot read {}: {e}", source_path_text(path)))?;
        entries.push((relative.clone(), data));
    }
    let bytes = tar(&entries)?;
    let out_dir = dir.join(OUTPUT_DIR);
    std::fs::create_dir_all(&out_dir).map_err(|e| format!("cannot create {}: {e}", source_path_text(&out_dir)))?;
    let path = out_dir.join(format!("{}-{version}.tar", manifest.name));
    std::fs::write(&path, &bytes).map_err(|e| format!("cannot write {}: {e}", source_path_text(&path)))?;
    let mut hasher = burdin_sha256::Sha256::new();
    hasher.update(&bytes);
    Ok(Packaged { path, files: entries.len(), checksum: format!("sha256:{}", hasher.finish_hex()) })
}

/// The files of the package in `dir` that its manifest selects, as `(relative path, file)` sorted by the relative
/// path (PKG21). `burdin.toml` is always selected, and the archive directory never is.
pub fn selected_files(dir: &Path, manifest: &Manifest) -> Result<Vec<(String, PathBuf)>, String> {
    let mut found = Vec::new();
    walk(dir, "", &mut found)?;
    found.sort();
    let mut selected = Vec::new();
    for (relative, path, is_link) in found {
        if !keep(manifest, &relative) {
            continue;
        }
        if is_link {
            return Err(format!(
                "cannot package `{relative}`: it is a symbolic link, and an archive holds regular files only"
            ));
        }
        selected.push((relative, path));
    }
    Ok(selected)
}

/// Whether the file at `relative` is in the archive (PKG21).
fn keep(manifest: &Manifest, relative: &str) -> bool {
    if relative == "burdin.toml" {
        return true;
    }
    if relative == OUTPUT_DIR || relative.starts_with(&format!("{OUTPUT_DIR}/")) {
        return false;
    }
    let chosen = match &manifest.include {
        Some(patterns) => patterns.iter().any(|p| pattern_selects(p, relative)),
        None => {
            let top = relative.split('/').next().unwrap_or(relative);
            !DEFAULT_EXCLUDED.contains(&top)
        }
    };
    chosen && !manifest.exclude.iter().any(|p| pattern_selects(p, relative))
}

/// Every file under `dir`, as `(relative path, path, is a symbolic link)`. Directories are walked, never listed.
fn walk(dir: &Path, prefix: &str, found: &mut Vec<(String, PathBuf, bool)>) -> Result<(), String> {
    let entries = std::fs::read_dir(dir).map_err(|e| format!("cannot read {}: {e}", source_path_text(dir)))?;
    for entry in entries {
        let entry = entry.map_err(|e| format!("cannot read {}: {e}", source_path_text(dir)))?;
        let name = entry.file_name();
        let Some(name) = name.to_str() else {
            return Err(format!("cannot package `{}`: its name is not UTF-8", source_path_text(&entry.path())));
        };
        let relative = if prefix.is_empty() { name.to_string() } else { format!("{prefix}/{name}") };
        let file_type =
            entry.file_type().map_err(|e| format!("cannot read {}: {e}", source_path_text(&entry.path())))?;
        if file_type.is_dir() {
            walk(&entry.path(), &relative, found)?;
        } else {
            found.push((relative, entry.path(), file_type.is_symlink()));
        }
    }
    Ok(())
}

/// Whether `pattern` selects `path` or a directory containing it (PKG21): the pattern is matched segment by segment
/// against the path's leading segments, so `docs` selects `docs/a.md` and `src/*.bd` does not select `src/a/b.bd`.
pub fn pattern_selects(pattern: &str, path: &str) -> bool {
    let pattern: Vec<&str> = pattern.trim_end_matches('/').split('/').collect();
    let segments: Vec<&str> = path.split('/').collect();
    (1..=segments.len()).any(|n| segments_match(&pattern, &segments[..n]))
}

/// Matches whole path segments: `**` matches zero or more of them, and every other segment matches as `wildcard`.
fn segments_match(pattern: &[&str], path: &[&str]) -> bool {
    match pattern.split_first() {
        None => path.is_empty(),
        Some((&"**", rest)) => (0..=path.len()).any(|skip| segments_match(rest, &path[skip..])),
        Some((first, rest)) => match path.split_first() {
            Some((segment, path_rest)) => wildcard_match(first, segment) && segments_match(rest, path_rest),
            None => false,
        },
    }
}

/// `*` matches any run of characters and `?` one character, within one segment.
fn wildcard_match(pattern: &str, text: &str) -> bool {
    let pattern: Vec<char> = pattern.chars().collect();
    let text: Vec<char> = text.chars().collect();
    chars_match(&pattern, &text)
}

fn chars_match(pattern: &[char], text: &[char]) -> bool {
    match pattern.split_first() {
        None => text.is_empty(),
        Some(('*', rest)) => (0..=text.len()).any(|skip| chars_match(rest, &text[skip..])),
        Some(('?', rest)) => !text.is_empty() && chars_match(rest, &text[1..]),
        Some((c, rest)) => text.first() == Some(c) && chars_match(rest, &text[1..]),
    }
}

/// The USTAR archive of `entries` (relative path, contents), in the order given. The caller sorts them (PKG22).
pub fn tar(entries: &[(String, Vec<u8>)]) -> Result<Vec<u8>, String> {
    let mut out = Vec::new();
    for (path, data) in entries {
        out.extend_from_slice(&header(path, data.len())?);
        out.extend_from_slice(data);
        let padding = (512 - data.len() % 512) % 512;
        out.resize(out.len() + padding, 0);
    }
    out.resize(out.len() + 1024, 0);
    Ok(out)
}

/// The 512-byte header of a regular file of `size` bytes at `path` (PKG22).
fn header(path: &str, size: usize) -> Result<[u8; 512], String> {
    let Some((prefix, name)) = split_name(path) else {
        return Err(format!(
            "`{path}` is too long for the archive: a path is split at a `/` into a prefix of at most 155 bytes and a \
             name of at most 100 bytes"
        ));
    };
    let mut h = [0u8; 512];
    put(&mut h[0..100], name.as_bytes());
    put(&mut h[100..108], b"0000644\0");
    put(&mut h[108..116], b"0000000\0");
    put(&mut h[116..124], b"0000000\0");
    put(&mut h[124..136], format!("{size:011o}\0").as_bytes());
    put(&mut h[136..148], b"00000000000\0");
    // The checksum is the sum of every header byte with the checksum field read as spaces.
    h[148..156].copy_from_slice(b"        ");
    h[156] = b'0';
    h[257..263].copy_from_slice(b"ustar\0");
    h[263..265].copy_from_slice(b"00");
    put(&mut h[329..337], b"0000000\0");
    put(&mut h[337..345], b"0000000\0");
    put(&mut h[345..500], prefix.as_bytes());
    let sum: u32 = h.iter().map(|&b| u32::from(b)).sum();
    h[148..156].copy_from_slice(format!("{sum:06o}\0 ").as_bytes());
    Ok(h)
}

/// Copies `bytes` into the start of `field`; the rest of the field stays zero.
fn put(field: &mut [u8], bytes: &[u8]) {
    field[..bytes.len()].copy_from_slice(bytes);
}

/// The `(prefix, name)` that stores `path`: the path itself when it fits the name field, otherwise the rightmost
/// split at a `/` whose parts fit the prefix and name fields.
fn split_name(path: &str) -> Option<(&str, &str)> {
    if path.len() <= 100 {
        return Some(("", path));
    }
    path.match_indices('/')
        .rev()
        .map(|(at, _)| (&path[..at], &path[at + 1..]))
        .find(|(prefix, name)| prefix.len() <= 155 && !name.is_empty() && name.len() <= 100)
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::fs;
    use std::time::{Duration, SystemTime};

    struct Scratch(PathBuf);

    impl Scratch {
        fn new(tag: &str) -> Scratch {
            use std::sync::atomic::{AtomicUsize, Ordering};
            static NEXT: AtomicUsize = AtomicUsize::new(0);
            let dir = std::env::temp_dir().join(format!(
                "burdin-archive-{tag}-{}-{}",
                std::process::id(),
                NEXT.fetch_add(1, Ordering::Relaxed)
            ));
            fs::create_dir_all(&dir).unwrap();
            Scratch(dir)
        }

        fn write(&self, relative: &str, text: &str) {
            let path = self.0.join(relative);
            fs::create_dir_all(path.parent().unwrap()).unwrap();
            fs::write(path, text).unwrap();
        }
    }

    impl Drop for Scratch {
        fn drop(&mut self) {
            let _ = fs::remove_dir_all(&self.0);
        }
    }

    fn manifest(extra: &str) -> Manifest {
        let text = format!("[package]\nname = \"app\"\nversion = \"0.1.0\"\n{extra}");
        burdin_query::package::parse_manifest_text(&text, Path::new("burdin.toml")).unwrap()
    }

    /// The relative paths `selected_files` picks, in order.
    fn selected(s: &Scratch, extra: &str) -> Vec<String> {
        selected_files(&s.0, &manifest(extra)).unwrap().into_iter().map(|(relative, _)| relative).collect()
    }

    #[test]
    fn patterns_match_segments_and_directories() {
        assert!(pattern_selects("src/**", "src/a/b.bd"));
        assert!(pattern_selects("src/**", "src/x.bd"));
        assert!(!pattern_selects("src/**", "lib/x.bd"));
        // A pattern anchored at the package root: `*.md` is not `docs/x.md`, but `**/*.md` is.
        assert!(pattern_selects("*.md", "README.md"));
        assert!(!pattern_selects("*.md", "docs/x.md"));
        assert!(pattern_selects("**/*.md", "docs/x.md"));
        assert!(pattern_selects("**/*.md", "README.md"));
        // A directory name selects what is under it, and a trailing slash changes nothing.
        assert!(pattern_selects("docs", "docs/a.md"));
        assert!(pattern_selects("docs/", "docs/a/b.md"));
        assert!(!pattern_selects("doc", "docs/a.md"));
        // `?` is one character, `*` stays within a segment.
        assert!(pattern_selects("src/?.bd", "src/a.bd"));
        assert!(!pattern_selects("src/?.bd", "src/ab.bd"));
        // `src/*` names the directory `src/a`, which holds `src/a/b.bd`; `src/*.bd` names no directory.
        assert!(pattern_selects("src/*", "src/a/b.bd"));
        assert!(!pattern_selects("src/*.bd", "src/a/b.bd"));
    }

    #[test]
    fn selection_defaults_and_overrides() {
        let s = Scratch::new("select");
        for file in [
            "burdin.toml",
            "src/main.bd",
            "src/util/strings.bd",
            "README.md",
            "docs/guide.md",
            "notes.txt",
            ".git/HEAD",
            "target/burdin/debug/out",
            "target/package/old-0.1.0.tar",
            "vendor/dep/burdin.toml",
        ] {
            s.write(file, "x");
        }
        // Without `include`, the top-level `.git`, `target`, and `vendor` directories are left out.
        assert_eq!(
            selected(&s, ""),
            ["README.md", "burdin.toml", "docs/guide.md", "notes.txt", "src/main.bd", "src/util/strings.bd"]
        );
        // `include` replaces the default: only what it names, and the directories above are reachable by name.
        assert_eq!(
            selected(&s, "include = [\"src/**\", \"README.md\"]\n"),
            ["README.md", "burdin.toml", "src/main.bd", "src/util/strings.bd"]
        );
        assert_eq!(selected(&s, "include = [\"vendor/**\"]\n"), ["burdin.toml", "vendor/dep/burdin.toml"]);
        // `exclude` removes from whatever was chosen.
        assert_eq!(
            selected(&s, "exclude = [\"docs\", \"src/util\"]\n"),
            ["README.md", "burdin.toml", "notes.txt", "src/main.bd"]
        );
        // The archive directory is never selected, even when a pattern names it.
        assert!(!selected(&s, "include = [\"target/**\"]\n").iter().any(|f| f.starts_with("target/package")));
    }

    #[test]
    fn tar_header_layout() {
        let archive = tar(&[("src/main.bd".to_string(), b"fn main() {}\n".to_vec())]).unwrap();
        // One header, one padded data block, and the two zero blocks that end the archive.
        assert_eq!(archive.len(), 512 * 4);
        assert_eq!(&archive[0..12], b"src/main.bd\0");
        assert_eq!(&archive[100..108], b"0000644\0");
        assert_eq!(&archive[108..116], b"0000000\0");
        assert_eq!(&archive[116..124], b"0000000\0");
        // `fn main() {}\n` is 13 bytes: 15 in octal.
        assert_eq!(&archive[124..136], b"00000000015\0");
        assert_eq!(&archive[136..148], b"00000000000\0");
        assert_eq!(archive[156], b'0');
        assert_eq!(&archive[257..263], b"ustar\0");
        assert_eq!(&archive[263..265], b"00");
        assert_eq!(&archive[512..525], b"fn main() {}\n");
        assert!(archive[525..1024].iter().all(|&b| b == 0));
        assert!(archive[1024..].iter().all(|&b| b == 0));
        // The checksum field holds the sum of the header with that field read as spaces.
        let mut header = archive[..512].to_vec();
        let stored = std::str::from_utf8(&header[148..154]).unwrap().to_string();
        header[148..156].copy_from_slice(b"        ");
        let sum: u32 = header.iter().map(|&b| u32::from(b)).sum();
        assert_eq!(stored, format!("{sum:06o}"));

        // A long path is split at a `/`: the prefix holds the start, the name the rest.
        let long = format!("{}/{}", "d".repeat(120), "f.bd");
        let archive = tar(&[(long.clone(), Vec::new())]).unwrap();
        assert_eq!(&archive[345..345 + 120], &long.as_bytes()[..120]);
        assert_eq!(&archive[0..4], b"f.bd");
        let too_long = format!("{}/{}", "d".repeat(200), "e".repeat(120));
        assert!(tar(&[(too_long, Vec::new())]).unwrap_err().contains("too long for the archive"));
    }

    #[test]
    fn identical_trees_give_identical_bytes() {
        let s = Scratch::new("repro");
        s.write("burdin.toml", "[package]\nname = \"app\"\nversion = \"0.1.0\"\n");
        s.write("src/main.bd", "fn main() {}\n");
        s.write("src/b.bd", "fn b() {}\n");
        let first = package(&s.0).unwrap();
        let bytes = fs::read(&first.path).unwrap();
        // Modification times are not part of the archive: a touched tree packs to the same bytes.
        let later = SystemTime::UNIX_EPOCH + Duration::from_secs(1_900_000_000);
        for file in ["burdin.toml", "src/main.bd", "src/b.bd"] {
            fs::OpenOptions::new().write(true).open(s.0.join(file)).unwrap().set_modified(later).unwrap();
        }
        let second = package(&s.0).unwrap();
        assert_eq!(first.checksum, second.checksum);
        assert_eq!(fs::read(&second.path).unwrap(), bytes);
        assert_eq!(first.files, 3);
        assert!(first.checksum.starts_with("sha256:") && first.checksum.len() == 7 + 64);
        assert!(second.path.ends_with("target/package/app-0.1.0.tar"), "{}", second.path.display());
    }

    #[test]
    fn a_package_without_a_version_is_refused() {
        let s = Scratch::new("noversion");
        s.write("burdin.toml", "[package]\nname = \"app\"\n");
        let error = package(&s.0).unwrap_err();
        assert!(error.contains("needs a `version` in [package]"), "{error}");
        assert!(!s.0.join("target").exists());
    }
}
