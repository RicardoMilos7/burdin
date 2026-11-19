//! Where releases come from and how they are fetched (`spec/toolchain.md` TC4, TC5): the channel's latest version,
//! a release's `SHA256SUMS`, and archives downloaded resumably and verified before anything uses them.

use std::fs;
use std::io::{Read, Seek, SeekFrom, Write};
use std::path::{Path, PathBuf};
use std::process::Command;

use burdin_json::json::Json;
use burdin_sha256::Sha256;

use crate::home::io_error;
use crate::version::{Channel, Version};

/// The GitHub repository the releases are published from (the same one `install.sh` names).
pub const REPO: &str = "RicardoMilos7/burdin";

/// The release source: GitHub, or a distribution root named by `BURDIN_DIST_ROOT` (a local directory, a `file://`
/// URL, or an `http(s)://` URL) laid out as `channels/<channel>` and `v<version>/<file>`.
pub enum Source {
    GitHub,
    Root(Location),
}

/// A place files are read from: a directory, or a URL fetched with `curl`.
#[derive(Clone, Debug)]
pub enum Location {
    Dir(PathBuf),
    Url(String),
}

impl Location {
    fn join(&self, relative: &str) -> Location {
        match self {
            Location::Dir(dir) => Location::Dir(dir.join(relative)),
            Location::Url(url) => Location::Url(format!("{}/{relative}", url.trim_end_matches('/'))),
        }
    }

    fn describe(&self) -> String {
        match self {
            Location::Dir(path) => path.display().to_string(),
            Location::Url(url) => url.clone(),
        }
    }
}

impl Source {
    pub fn from_env() -> Source {
        match std::env::var("BURDIN_DIST_ROOT") {
            Ok(root) if !root.is_empty() => {
                if let Some(path) = root.strip_prefix("file://") {
                    Source::Root(Location::Dir(PathBuf::from(path)))
                } else if root.starts_with("http://") || root.starts_with("https://") {
                    Source::Root(Location::Url(root))
                } else {
                    Source::Root(Location::Dir(PathBuf::from(root)))
                }
            }
            _ => Source::GitHub,
        }
    }

    /// The latest version of `channel`.
    pub fn latest(&self, channel: Channel) -> Result<Version, String> {
        let text = match self {
            Source::Root(root) => {
                let location = root.join(&format!("channels/{}", channel.name()));
                fetch_text(&location)?.trim().to_string()
            }
            Source::GitHub => github_latest(channel)?,
        };
        Version::parse(&text)
            .ok_or_else(|| format!("the {} channel names `{text}`, which is not a version", channel.name()))
    }

    /// A file of the release `version`.
    pub fn release_file(&self, version: &Version, file: &str) -> Location {
        match self {
            Source::Root(root) => root.join(&format!("v{version}/{file}")),
            Source::GitHub => Location::Url(format!("https://github.com/{REPO}/releases/download/v{version}/{file}")),
        }
    }

    /// The SHA-256 the release's `SHA256SUMS` lists for `file`, or `None` when it does not list it.
    pub fn expected_sha256(&self, version: &Version, file: &str) -> Result<Option<String>, String> {
        let location = self.release_file(version, "SHA256SUMS");
        let sums = fetch_text(&location).map_err(|e| format!("release {version}: {e}"))?;
        Ok(find_sum(&sums, file))
    }
}

/// The checksum `sums` (the `sha256sum` format, `<hex>  <name>` or `<hex> *<name>`) lists for `file`.
pub fn find_sum(sums: &str, file: &str) -> Option<String> {
    sums.lines().find_map(|line| {
        let (hash, name) = line.split_once(char::is_whitespace)?;
        let name = name.trim_start().trim_start_matches('*').trim_end();
        (name == file && hash.len() == 64 && hash.bytes().all(|b| b.is_ascii_hexdigit()))
            .then(|| hash.to_ascii_lowercase())
    })
}

/// The latest release of a channel on GitHub, from the releases API (as `install.sh` resolves it).
fn github_latest(channel: Channel) -> Result<String, String> {
    let api = format!("https://api.github.com/repos/{REPO}/releases");
    let tag_of = |release: &Json| release.get("tag_name").as_str().map(|tag| tag.trim_start_matches('v').to_string());
    match channel {
        Channel::Stable => {
            let json = Json::parse(&fetch_text(&Location::Url(format!("{api}/latest")))?)
                .map_err(|e| format!("the GitHub releases API answered with invalid JSON: {e}"))?;
            tag_of(&json).ok_or_else(|| "GitHub reports no latest stable release".to_string())
        }
        Channel::Nightly => {
            let json = Json::parse(&fetch_text(&Location::Url(format!("{api}?per_page=30")))?)
                .map_err(|e| format!("the GitHub releases API answered with invalid JSON: {e}"))?;
            json.as_array()
                .iter()
                .filter_map(tag_of)
                .find(|tag| tag.contains("nightly"))
                .ok_or_else(|| "GitHub lists no nightly release".to_string())
        }
    }
}

/// The whole text at `location`.
pub fn fetch_text(location: &Location) -> Result<String, String> {
    match location {
        Location::Dir(path) => fs::read_to_string(path).map_err(|e| io_error("read", path, e)),
        Location::Url(url) => {
            let output = curl()
                .args(["-fsSL", "--retry", "2", "-H", "User-Agent: burdin-toolchain", url])
                .output()
                .map_err(|e| format!("cannot run curl to fetch {url}: {e}"))?;
            if !output.status.success() {
                return Err(format!("cannot fetch {url}: {}", curl_failure(&output.stderr, output.status.code())));
            }
            String::from_utf8(output.stdout).map_err(|_| format!("{url} is not UTF-8 text"))
        }
    }
}

fn curl() -> Command {
    Command::new(if cfg!(windows) { "curl.exe" } else { "curl" })
}

fn curl_failure(stderr: &[u8], code: Option<i32>) -> String {
    let text = String::from_utf8_lossy(stderr).trim().to_string();
    if text.is_empty() { format!("curl exited with status {}", code.unwrap_or(-1)) } else { text }
}

/// Downloads `location` into `part`. With `resume`, the bytes already in `part` are kept and only the rest is
/// fetched (an HTTP range request through `curl -C -`, or the remaining bytes of a local file).
fn download(location: &Location, part: &Path, resume: bool) -> Result<(), String> {
    match location {
        Location::Dir(path) => {
            let mut source = fs::File::open(path).map_err(|e| io_error("read", path, e))?;
            let offset = if resume { fs::metadata(part).map(|m| m.len()).unwrap_or(0) } else { 0 };
            let size = source.metadata().map_err(|e| io_error("read", path, e))?.len();
            let mut target = if resume && offset <= size {
                source.seek(SeekFrom::Start(offset)).map_err(|e| io_error("read", path, e))?;
                fs::OpenOptions::new().append(true).open(part).map_err(|e| io_error("write", part, e))?
            } else {
                fs::File::create(part).map_err(|e| io_error("write", part, e))?
            };
            let mut buffer = vec![0u8; 1 << 16];
            loop {
                let read = source.read(&mut buffer).map_err(|e| io_error("read", path, e))?;
                if read == 0 {
                    break;
                }
                target.write_all(&buffer[..read]).map_err(|e| io_error("write", part, e))?;
            }
            Ok(())
        }
        Location::Url(url) => {
            let mut command = curl();
            command.args(["-fsSL", "--retry", "2", "-H", "User-Agent: burdin-toolchain"]);
            if resume {
                command.args(["-C", "-"]);
            }
            let output = command
                .arg("-o")
                .arg(part)
                .arg(url)
                .output()
                .map_err(|e| format!("cannot run curl to download {url}: {e}"))?;
            if output.status.success() {
                Ok(())
            } else {
                Err(format!("cannot download {url}: {}", curl_failure(&output.stderr, output.status.code())))
            }
        }
    }
}

pub fn sha256_of(path: &Path) -> Result<String, String> {
    let mut file = fs::File::open(path).map_err(|e| io_error("read", path, e))?;
    let mut hasher = Sha256::new();
    let mut buffer = vec![0u8; 1 << 16];
    loop {
        let read = file.read(&mut buffer).map_err(|e| io_error("read", path, e))?;
        if read == 0 {
            break;
        }
        hasher.update(&buffer[..read]);
    }
    Ok(hasher.finish_hex())
}

/// Downloads `file` of release `version` into `downloads` and verifies it against `expected` (TC5). A partial
/// download left by an interrupted run is resumed; if the resumed file does not verify, it is discarded and fetched
/// again from the start. A file that still does not verify is deleted and is an error, so nothing unverified is
/// ever unpacked. Returns the verified file, which the caller removes once it is installed.
pub fn fetch_verified(
    source: &Source,
    version: &Version,
    file: &str,
    expected: &str,
    downloads: &Path,
) -> Result<PathBuf, String> {
    fs::create_dir_all(downloads).map_err(|e| io_error("create", downloads, e))?;
    let location = source.release_file(version, file);
    let part = downloads.join(format!("{file}.part"));
    if part.is_file() {
        if sha256_of(&part)? == expected {
            return Ok(part);
        }
        eprintln!("info: resuming the download of {file}");
        if download(&location, &part, true).is_ok() && sha256_of(&part)? == expected {
            return Ok(part);
        }
        eprintln!("info: the resumed download of {file} did not verify; downloading it again");
        let _ = fs::remove_file(&part);
    } else {
        eprintln!("info: downloading {}", location.describe());
    }
    // A failed transfer keeps what arrived, so the next run resumes it.
    download(&location, &part, false)?;
    let actual = sha256_of(&part)?;
    if actual != expected {
        let _ = fs::remove_file(&part);
        return Err(format!("checksum mismatch for {file}: expected {expected}, got {actual}"));
    }
    Ok(part)
}

/// Unpacks the gzip-compressed tar `archive` into `into` with the system `tar`. On Windows that is the system's
/// bsdtar, because Git's GNU tar can come first on `PATH` and reads `C:\...` as a remote archive.
pub fn extract(archive: &Path, into: &Path) -> Result<(), String> {
    fs::create_dir_all(into).map_err(|e| io_error("create", into, e))?;
    let mut tar = PathBuf::from("tar");
    if cfg!(windows)
        && let Some(root) = std::env::var_os("SystemRoot")
    {
        let system = PathBuf::from(root).join("System32").join("tar.exe");
        if system.is_file() {
            tar = system;
        }
    }
    let output = Command::new(&tar)
        .arg("-xzf")
        .arg(archive)
        .arg("-C")
        .arg(into)
        .output()
        .map_err(|e| format!("cannot run {}: {e}", tar.display()))?;
    if !output.status.success() {
        let name = archive.file_name().map(|n| n.to_string_lossy().into_owned()).unwrap_or_default();
        return Err(format!("cannot unpack {name}: {}", String::from_utf8_lossy(&output.stderr).trim()));
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn finds_a_file_in_sha256sums() {
        let hash = "a".repeat(64);
        let sums = format!("{hash}  burdin-0.4.1-x.tar.gz\n{}  *other\n", "B".repeat(64));
        assert_eq!(find_sum(&sums, "burdin-0.4.1-x.tar.gz"), Some(hash));
        assert_eq!(find_sum(&sums, "other"), Some("b".repeat(64)));
        assert_eq!(find_sum(&sums, "missing"), None);
        assert_eq!(find_sum("abc  short\n", "short"), None);
    }

    #[test]
    fn resumes_and_verifies_a_local_download() {
        let dir = std::env::temp_dir().join(format!("burdin-toolchain-dist-{}", std::process::id()));
        let _ = fs::remove_dir_all(&dir);
        let release = dir.join("root/v1.0.0");
        fs::create_dir_all(&release).unwrap();
        let data: Vec<u8> = (0..100_000u32).map(|i| (i % 251) as u8).collect();
        fs::write(release.join("a.tar.gz"), &data).unwrap();
        let expected = sha256_of(&release.join("a.tar.gz")).unwrap();
        let source = Source::Root(Location::Dir(dir.join("root")));
        let version = Version::parse("1.0.0").unwrap();
        let downloads = dir.join("downloads");
        fs::create_dir_all(&downloads).unwrap();

        // A correct prefix is resumed.
        fs::write(downloads.join("a.tar.gz.part"), &data[..30_000]).unwrap();
        let got = fetch_verified(&source, &version, "a.tar.gz", &expected, &downloads).unwrap();
        assert_eq!(fs::read(&got).unwrap(), data);

        // A corrupt prefix is discarded and fetched again.
        let mut corrupt = data[..30_000].to_vec();
        corrupt[5] ^= 0xff;
        fs::write(downloads.join("a.tar.gz.part"), &corrupt).unwrap();
        let got = fetch_verified(&source, &version, "a.tar.gz", &expected, &downloads).unwrap();
        assert_eq!(fs::read(&got).unwrap(), data);

        // A wrong checksum is an error and leaves nothing behind.
        fs::remove_file(&got).unwrap();
        let error = fetch_verified(&source, &version, "a.tar.gz", &"0".repeat(64), &downloads).unwrap_err();
        assert!(error.starts_with("checksum mismatch for a.tar.gz"), "{error}");
        assert!(!downloads.join("a.tar.gz.part").exists());
        fs::remove_dir_all(&dir).unwrap();
    }
}
