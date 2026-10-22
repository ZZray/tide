//! Self-update from GitHub Releases.
//!
//! The release list comes from the public Atom feed (`/releases.atom`) instead of the REST API:
//! the anonymous API limit (60 requests/hour per IP) is easily exhausted behind shared proxies.
//! Assets are downloaded from fixed `releases/download/<tag>/` URLs over HTTPS and must match
//! `SHA256SUMS.txt` published with the same release before the installer is started.
//! Only the Windows installer build updates itself; other platforms keep the pure helpers.
#![cfg_attr(not(windows), allow(dead_code))]

use anyhow::{ensure, Context, Result};
use serde::Serialize;
use sha2::{Digest, Sha256};
use std::{
    io::{Read, Write},
    path::{Path, PathBuf},
};

pub const REPOSITORY: &str = "https://github.com/ZZray/tide";
/// Upper bound for a downloaded installer; real packages are below 20 MB.
const MAX_INSTALLER_BYTES: u64 = 256 * 1024 * 1024;
const MAX_TEXT_BYTES: u64 = 4 * 1024 * 1024;

#[derive(Clone, Debug, PartialEq, Eq, Serialize)]
pub struct Release {
    pub version: String,
    pub page: String,
    pub installer_name: String,
    pub installer_url: String,
    pub sums_url: String,
}

impl Release {
    pub fn new(repository: &str, version: &semver::Version) -> Self {
        let tag = format!("v{version}");
        let installer_name = format!("Tide-{tag}-windows-x64-setup.exe");
        Self {
            version: version.to_string(),
            page: format!("{repository}/releases/tag/{tag}"),
            installer_url: format!("{repository}/releases/download/{tag}/{installer_name}"),
            sums_url: format!("{repository}/releases/download/{tag}/SHA256SUMS.txt"),
            installer_name,
        }
    }
}

#[derive(Clone, Copy, Debug)]
pub enum InstallMode {
    /// Interactive setup wizard.
    Wizard,
    /// In-app update: no prompts, then start Tide again (tray only when `background`).
    Silent { background: bool },
}

/// An installer whose bytes matched the release checksum when it was downloaded.
#[derive(Clone, Debug)]
pub struct Download {
    pub path: PathBuf,
    pub sha256: String,
}

pub fn current_version() -> semver::Version {
    semver::Version::parse(env!("CARGO_PKG_VERSION")).expect("package version is semver")
}

/// Highest release version linked from a GitHub releases Atom feed of `repository`.
pub fn latest_in_feed(feed: &str, repository: &str) -> Option<semver::Version> {
    let prefix = format!("href=\"{repository}/releases/tag/v");
    let mut latest: Option<semver::Version> = None;
    let mut rest = feed;
    while let Some(start) = rest.find(&prefix) {
        rest = &rest[start + prefix.len()..];
        let Some(end) = rest.find('"') else { break };
        if let Ok(version) = semver::Version::parse(&rest[..end]) {
            if latest.as_ref().is_none_or(|l| version > *l) {
                latest = Some(version);
            }
        }
        rest = &rest[end..];
    }
    latest
}

/// Lowercase hex SHA-256 recorded for `name` in a `sha256sum`-style manifest.
pub fn expected_hash(sums: &str, name: &str) -> Option<String> {
    sums.lines().find_map(|line| {
        let (hash, file) = line.trim().split_once(char::is_whitespace)?;
        let file = file.trim_start().trim_start_matches('*');
        (file == name && hash.len() == 64 && hash.bytes().all(|b| b.is_ascii_hexdigit()))
            .then(|| hash.to_ascii_lowercase())
    })
}

/// Stream `reader` into `writer`, refusing more than `limit` bytes, and require the SHA-256
/// of everything copied to equal `expected` (lowercase hex).
pub fn copy_verified(
    reader: &mut impl Read,
    writer: &mut impl Write,
    limit: u64,
    expected: &str,
) -> Result<u64> {
    let mut hasher = Sha256::new();
    let mut buffer = vec![0; 64 * 1024];
    let mut total = 0u64;
    loop {
        let read = reader.read(&mut buffer)?;
        if read == 0 {
            break;
        }
        total += read as u64;
        ensure!(total <= limit, "download is larger than {limit} bytes");
        hasher.update(&buffer[..read]);
        writer.write_all(&buffer[..read])?;
    }
    let actual = hex(&hasher.finalize());
    ensure!(
        actual == expected,
        "SHA-256 mismatch: expected {expected}, got {actual}"
    );
    Ok(total)
}

pub fn file_sha256(path: &Path) -> Result<String> {
    let mut file = std::fs::File::open(path)?;
    let mut hasher = Sha256::new();
    std::io::copy(&mut file, &mut hasher)?;
    Ok(hex(&hasher.finalize()))
}

fn hex(bytes: &[u8]) -> String {
    bytes.iter().map(|b| format!("{b:02x}")).collect()
}

/// Installation directory when running from the Inno Setup install (it places `unins000.exe`
/// next to the executable). `None` for portable copies, which are updated by hand.
pub fn installed_dir() -> Option<PathBuf> {
    let dir = std::env::current_exe().ok()?.parent()?.to_path_buf();
    dir.join("unins000.exe").is_file().then_some(dir)
}

pub fn cache_dir() -> Result<PathBuf> {
    Ok(directories::ProjectDirs::from("org", "Tide", "Tide")
        .context("cannot determine user cache directory")?
        .cache_dir()
        .join("updates"))
}

/// Append one line to `updates/update.log` so failed checks on other machines can be diagnosed.
pub fn log(line: &str) {
    let Ok(dir) = cache_dir() else { return };
    if std::fs::create_dir_all(&dir).is_err() {
        return;
    }
    if let Ok(mut file) = std::fs::OpenOptions::new()
        .create(true)
        .append(true)
        .open(dir.join("update.log"))
    {
        let _ = writeln!(
            file,
            "{} {line}",
            chrono::Local::now().format("%Y-%m-%dT%H:%M:%S%:z")
        );
    }
}

#[cfg(windows)]
pub use net::{check, download, launch_installer};

#[cfg(windows)]
mod net {
    use super::*;
    use anyhow::bail;
    use std::time::{Duration, Instant};

    fn agent() -> ureq::Agent {
        use ureq::tls::{RootCerts, TlsConfig, TlsProvider};
        // Windows certificate store and the user's proxy (environment or system settings).
        ureq::Agent::config_builder()
            .https_only(true)
            .user_agent(concat!("Tide/", env!("CARGO_PKG_VERSION")))
            .timeout_connect(Some(Duration::from_secs(15)))
            .timeout_recv_response(Some(Duration::from_secs(30)))
            .timeout_per_read(Some(Duration::from_secs(60)))
            .tls_config(
                TlsConfig::builder()
                    .provider(TlsProvider::NativeTls)
                    .root_certs(RootCerts::PlatformVerifier)
                    .build(),
            )
            .build()
            .new_agent()
    }

    fn text(agent: &ureq::Agent, url: &str) -> Result<String> {
        agent
            .get(url)
            .call()
            .with_context(|| format!("request failed: {url}"))?
            .body_mut()
            .with_config()
            .limit(MAX_TEXT_BYTES)
            .read_to_string()
            .with_context(|| format!("cannot read response: {url}"))
    }

    /// The newest published release if it is newer than this build.
    pub fn check() -> Result<Option<Release>> {
        let started = Instant::now();
        let result = (|| {
            let feed = text(&agent(), &format!("{REPOSITORY}/releases.atom"))?;
            let latest =
                latest_in_feed(&feed, REPOSITORY).context("release feed lists no versions")?;
            Ok((latest > current_version()).then(|| Release::new(REPOSITORY, &latest)))
        })();
        match &result {
            Ok(found) => log(&format!(
                "INFO update.check current={} latest={} elapsed_ms={}",
                current_version(),
                found.as_ref().map_or("none", |r| r.version.as_str()),
                started.elapsed().as_millis()
            )),
            Err(error) => log(&format!(
                "WARN update.check current={} error=\"{error:#}\" elapsed_ms={}",
                current_version(),
                started.elapsed().as_millis()
            )),
        }
        result
    }

    /// Download the installer into the update cache and verify it against `SHA256SUMS.txt`.
    /// A previously verified download of the same release is reused.
    pub fn download(release: &Release) -> Result<Download> {
        let started = Instant::now();
        let result = (|| -> Result<Download> {
            let agent = agent();
            let sums = text(&agent, &release.sums_url)
                .context("release checksums are not available yet")?;
            let expected = expected_hash(&sums, &release.installer_name)
                .context("installer is missing from SHA256SUMS.txt")?;
            let dir = cache_dir()?;
            std::fs::create_dir_all(&dir)?;
            let target = dir.join(&release.installer_name);
            if target.is_file() && file_sha256(&target)? == expected {
                return Ok(Download {
                    path: target,
                    sha256: expected,
                });
            }
            let mut response = agent
                .get(&release.installer_url)
                .call()
                .with_context(|| format!("request failed: {}", release.installer_url))?;
            let mut temp = tempfile::NamedTempFile::new_in(&dir)?;
            let mut reader = response
                .body_mut()
                .with_config()
                .limit(MAX_INSTALLER_BYTES)
                .reader();
            let bytes = copy_verified(
                &mut reader,
                temp.as_file_mut(),
                MAX_INSTALLER_BYTES,
                &expected,
            )?;
            temp.as_file().sync_all()?;
            temp.persist(&target).map_err(|e| e.error)?;
            // Keep only the verified installer for this release.
            for entry in std::fs::read_dir(&dir)?.flatten() {
                let path = entry.path();
                if path != target && path.extension().is_some_and(|e| e == "exe") {
                    let _ = std::fs::remove_file(path);
                }
            }
            log(&format!(
                "INFO update.download version={} bytes={bytes} sha256={expected}",
                release.version
            ));
            Ok(Download {
                path: target,
                sha256: expected,
            })
        })();
        match &result {
            Ok(download) => log(&format!(
                "INFO update.ready version={} path=\"{}\" elapsed_ms={}",
                release.version,
                download.path.display(),
                started.elapsed().as_millis()
            )),
            Err(error) => log(&format!(
                "WARN update.download version={} error=\"{error:#}\" elapsed_ms={}",
                release.version,
                started.elapsed().as_millis()
            )),
        }
        result
    }

    /// Start the installer detached. `Silent` waits for this process to exit, installs without
    /// prompts and starts Tide again (tray only when `background`); `Wizard` shows normal setup.
    pub fn launch_installer(download: &Download, mode: InstallMode) -> Result<()> {
        use std::os::windows::process::CommandExt;
        const DETACHED_PROCESS: u32 = 0x0000_0008;
        const CREATE_NEW_PROCESS_GROUP: u32 = 0x0000_0200;
        let installer = &download.path;
        ensure!(
            installer.is_file(),
            "installer not found: {}",
            installer.display()
        );
        // The file may have waited in the cache since it was verified.
        ensure!(
            file_sha256(installer)? == download.sha256,
            "installer changed after verification: {}",
            installer.display()
        );
        let mut command = std::process::Command::new(installer);
        if let InstallMode::Silent { background } = mode {
            let log_path = cache_dir()?.join("install.log");
            command.args(["/VERYSILENT", "/SUPPRESSMSGBOXES", "/NORESTART", "/SP-"]);
            command.arg(if background {
                "/RELAUNCH=background"
            } else {
                "/RELAUNCH=window"
            });
            command.arg(format!("/WAITPID={}", std::process::id()));
            command.arg(format!("/LOG={}", log_path.display()));
        }
        command.creation_flags(DETACHED_PROCESS | CREATE_NEW_PROCESS_GROUP);
        match command.spawn() {
            Ok(child) => {
                log(&format!(
                    "INFO update.install installer=\"{}\" mode={mode:?} pid={}",
                    installer.display(),
                    child.id()
                ));
                Ok(())
            }
            Err(error) => {
                log(&format!(
                    "WARN update.install installer=\"{}\" error=\"{error}\"",
                    installer.display()
                ));
                bail!("cannot start installer {}: {error}", installer.display())
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const FEED: &str = r#"<feed>
      <link type="text/html" rel="alternate" href="https://github.com/ZZray/tide/releases"/>
      <entry><link rel="alternate" type="text/html" href="https://github.com/ZZray/tide/releases/tag/v0.1.0"/></entry>
      <entry><link rel="alternate" type="text/html" href="https://github.com/ZZray/tide/releases/tag/v0.10.0-rc.1"/></entry>
      <entry><link rel="alternate" type="text/html" href="https://github.com/ZZray/tide/releases/tag/v0.2.0"/></entry>
      <entry><link rel="alternate" type="text/html" href="https://github.com/ZZray/tide/releases/tag/nightly"/></entry>
      <entry><link rel="alternate" type="text/html" href="https://github.com/other/tide/releases/tag/v9.0.0"/></entry>
    </feed>"#;

    #[test]
    fn feed_picks_highest_semver_of_this_repository_only() {
        let latest = latest_in_feed(FEED, REPOSITORY).unwrap();
        assert_eq!(latest, semver::Version::parse("0.10.0-rc.1").unwrap());
        assert!(latest_in_feed("<feed></feed>", REPOSITORY).is_none());
    }

    #[test]
    fn release_urls_follow_the_packaging_names() {
        let release = Release::new(REPOSITORY, &semver::Version::new(0, 1, 1));
        assert_eq!(release.installer_name, "Tide-v0.1.1-windows-x64-setup.exe");
        assert_eq!(
            release.installer_url,
            "https://github.com/ZZray/tide/releases/download/v0.1.1/Tide-v0.1.1-windows-x64-setup.exe"
        );
        assert_eq!(
            release.sums_url,
            "https://github.com/ZZray/tide/releases/download/v0.1.1/SHA256SUMS.txt"
        );
        assert_eq!(
            release.page,
            "https://github.com/ZZray/tide/releases/tag/v0.1.1"
        );
    }

    #[test]
    fn checksum_manifest_lookup_is_exact() {
        let sums = "F2847A8243CEF453A4A96C06E5C8DD5007586AF614DD1926AC55B6A1515235F9  Tide-v0.1.0-windows-x64-setup.exe\n\
                    2196d033372889649c26d0efd92e0162f3c2d1b8d44bfcc4cf6b5c004a45d6e9 *Tide-v0.1.0-windows-x64-portable.zip\n\
                    nothex  other.exe\n";
        assert_eq!(
            expected_hash(sums, "Tide-v0.1.0-windows-x64-setup.exe").unwrap(),
            "f2847a8243cef453a4a96c06e5c8dd5007586af614dd1926ac55b6a1515235f9"
        );
        assert!(expected_hash(sums, "Tide-v0.1.0-windows-x64-portable.zip").is_some());
        assert!(expected_hash(sums, "Tide-v0.1.0-windows-x64-setup").is_none());
        assert!(expected_hash(sums, "other.exe").is_none());
    }

    #[test]
    fn verified_copy_rejects_wrong_hash_and_oversize() {
        let data = b"installer bytes";
        let good = hex(&Sha256::digest(data));
        let mut out = Vec::new();
        assert_eq!(
            copy_verified(&mut &data[..], &mut out, 1024, &good).unwrap(),
            15
        );
        assert_eq!(out, data);
        let bad = "0".repeat(64);
        assert!(copy_verified(&mut &data[..], &mut Vec::new(), 1024, &bad)
            .unwrap_err()
            .to_string()
            .contains("mismatch"));
        assert!(copy_verified(&mut &data[..], &mut Vec::new(), 4, &good)
            .unwrap_err()
            .to_string()
            .contains("larger"));
    }

    #[test]
    fn current_version_matches_package() {
        assert_eq!(current_version().to_string(), env!("CARGO_PKG_VERSION"));
    }

    /// Real GitHub check and download of the published v0.1.0 installer. Opt in with
    /// `TIDE_REAL_NETWORK=1`; uses the user's proxy settings.
    #[cfg(windows)]
    #[test]
    fn real_github_feed_and_verified_download() {
        if std::env::var("TIDE_REAL_NETWORK").as_deref() != Ok("1") {
            return;
        }
        check().unwrap();
        let release = Release::new(REPOSITORY, &semver::Version::new(0, 1, 0));
        let first = download(&release).unwrap();
        let expected = "f2847a8243cef453a4a96c06e5c8dd5007586af614dd1926ac55b6a1515235f9";
        assert_eq!(first.sha256, expected);
        assert_eq!(file_sha256(&first.path).unwrap(), expected);
        // A second call reuses the verified file.
        assert_eq!(download(&release).unwrap().path, first.path);
        std::fs::remove_file(first.path).unwrap();
    }
}
