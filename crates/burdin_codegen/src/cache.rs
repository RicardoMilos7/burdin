//! The object cache (issue #243, `spec/target.md`, TG11-TG16): the object file of a codegen unit is kept under a key
//! that hashes everything its code generation reads, so a later build whose unit is the same module copies the object
//! instead of optimizing and compiling it again.
//!
//! The key is taken from the unit's bitcode just before the optimizer runs. At that point the unit is a function of
//! the program's typed tree and the profile's flags, and nothing else: a function another unit owns is a declaration
//! (or a small inlinable copy, which the bitcode then contains), so editing one function changes the keys of the
//! units that define or inline it and no others. Everything is best-effort: a cache that cannot be read or written
//! is a miss, never an error.

use std::collections::HashSet;
use std::path::{Path, PathBuf};

/// The first thing every key hashes. A change to how objects are produced from a module that this crate cannot see
/// (the pipeline text, the target machine's settings) bumps this, so older objects are never found again.
const FORMAT: &str = "burdin-object-v1";

/// Where the objects of one build target are kept, and the compiler identity every key includes.
#[derive(Clone, Debug)]
pub struct ObjectCache {
    /// The directory holding `<key>.<ext>` files. It is created when the first object is stored.
    pub dir: PathBuf,
    /// The compiler's identity (version, commit, LLVM version, and its own file), so a different compiler never
    /// finds this compiler's objects.
    pub identity: String,
}

impl ObjectCache {
    /// The key of a module whose bitcode is `bitcode`, compiled at `level` for `triple`.
    pub(crate) fn key(&self, bitcode: &[u8], level: u8, triple: &str) -> String {
        let mut hash = burdin_sha256::Sha256::new();
        hash.update(format!("{FORMAT}\n{}\nopt {level}\ntriple {triple}\n", self.identity).as_bytes());
        hash.update(bitcode);
        hash.finish_hex()
    }

    fn path(&self, key: &str, ext: &str) -> PathBuf {
        self.dir.join(format!("{key}.{ext}"))
    }

    /// Copies the cached object for `key` to `to`. False when there is none or it cannot be copied.
    pub(crate) fn fetch(&self, key: &str, to: &Path) -> bool {
        let ext = to.extension().and_then(|e| e.to_str()).unwrap_or("o");
        std::fs::copy(self.path(key, ext), to).is_ok()
    }

    /// Keeps the object at `from` as the object for `key`. It is written under a temporary name and renamed, so a
    /// build that is interrupted or runs beside another never leaves a half-written object under a key.
    pub(crate) fn store(&self, key: &str, from: &Path) {
        let ext = from.extension().and_then(|e| e.to_str()).unwrap_or("o");
        if std::fs::create_dir_all(&self.dir).is_err() {
            return;
        }
        let target = self.path(key, ext);
        let temp = self.dir.join(format!("{key}.tmp-{}.{ext}", std::process::id()));
        if std::fs::copy(from, &temp).and_then(|_| std::fs::rename(&temp, &target)).is_err() {
            let _ = std::fs::remove_file(&temp);
        }
    }

    /// Removes every object whose key is not in `keep` (TG15): the cache holds the objects of the last build, as the
    /// executable cache holds the last executable. Files that are not objects are left alone.
    pub(crate) fn prune(&self, keep: &HashSet<String>) {
        let Ok(entries) = std::fs::read_dir(&self.dir) else {
            return;
        };
        for entry in entries.flatten() {
            let path = entry.path();
            let is_object = path.extension().and_then(|e| e.to_str()).is_some_and(|e| e == "o" || e == "obj");
            let key = path.file_stem().and_then(|s| s.to_str()).unwrap_or("");
            if is_object && key.len() == 64 && !keep.contains(key) {
                let _ = std::fs::remove_file(&path);
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn scratch(tag: &str) -> PathBuf {
        let dir = std::env::temp_dir().join(format!("burdin-objcache-{tag}-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        dir
    }

    fn cache(dir: &Path) -> ObjectCache {
        ObjectCache { dir: dir.join("objects"), identity: "test-compiler".to_string() }
    }

    #[test]
    fn the_key_depends_on_every_input() {
        let dir = scratch("key");
        let cache = cache(&dir);
        let base = cache.key(b"module", 0, "x86_64-linux");
        assert_eq!(base.len(), 64);
        assert_eq!(base, cache.key(b"module", 0, "x86_64-linux"));
        assert_ne!(base, cache.key(b"module!", 0, "x86_64-linux"));
        assert_ne!(base, cache.key(b"module", 2, "x86_64-linux"));
        assert_ne!(base, cache.key(b"module", 0, "aarch64-linux"));
        let other = ObjectCache { identity: "another-compiler".to_string(), ..cache.clone() };
        assert_ne!(base, other.key(b"module", 0, "x86_64-linux"));
        let _ = std::fs::remove_dir_all(dir);
    }

    #[test]
    fn a_stored_object_is_fetched_and_pruned() {
        let dir = scratch("store");
        let cache = cache(&dir);
        let object = dir.join("unit.o");
        std::fs::write(&object, b"object bytes").unwrap();
        let (kept, dropped) = ("a".repeat(64), "b".repeat(64));
        let target = dir.join("fetched.o");
        assert!(!cache.fetch(&kept, &target), "an empty cache has no object");
        cache.store(&kept, &object);
        cache.store(&dropped, &object);
        assert!(cache.fetch(&kept, &target));
        assert_eq!(std::fs::read(&target).unwrap(), b"object bytes");
        std::fs::write(cache.dir.join("notes.txt"), "left alone").unwrap();
        cache.prune(&std::iter::once(kept.clone()).collect());
        assert!(!cache.fetch(&dropped, &target), "an object outside the keep set is removed");
        assert!(cache.fetch(&kept, &target));
        assert!(cache.dir.join("notes.txt").exists(), "a file that is not an object is left alone");
        let _ = std::fs::remove_dir_all(dir);
    }

    #[test]
    fn an_unwritable_cache_is_a_miss() {
        let dir = scratch("blocked");
        std::fs::write(dir.join("objects"), "a file where the directory belongs").unwrap();
        let cache = cache(&dir);
        let object = dir.join("unit.o");
        std::fs::write(&object, b"x").unwrap();
        cache.store(&"c".repeat(64), &object);
        assert!(!cache.fetch(&"c".repeat(64), &dir.join("out.o")));
        cache.prune(&HashSet::new());
        let _ = std::fs::remove_dir_all(dir);
    }
}
