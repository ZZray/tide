use anyhow::{bail, Result};
use git2::{FileFavor, IndexEntry, IndexTime, MergeFileOptions, Oid, Repository};
use serde::{Deserialize, Serialize};
use std::collections::{BTreeMap, BTreeSet};

#[derive(Clone, Copy, Debug, Default, Deserialize, Serialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum ConflictPolicy {
    #[default]
    KeepBoth,
    PreferLocal,
    PreferRemote,
    Ask,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Version {
    pub oid: Oid,
    pub mode: u32,
}
pub type Files = BTreeMap<String, Version>;

#[derive(Clone, Debug, Deserialize, Serialize)]
pub struct Conflict {
    pub path: String,
    pub local: String,
    pub remote: String,
    pub copy: Option<String>,
}

pub struct Reconciliation {
    pub files: Files,
    pub conflicts: Vec<Conflict>,
}

pub fn validate_path(path: &str) -> Result<()> {
    if path.is_empty() || path.contains(['\\', '\0', ':']) {
        bail!("unsafe or non-portable Git path: {path:?}");
    }
    for part in path.split('/') {
        let stem = part.split('.').next().unwrap_or("").to_ascii_uppercase();
        if part.is_empty()
            || matches!(part, "." | "..")
            || part.eq_ignore_ascii_case(".git")
            || part.ends_with(['.', ' '])
            || part.chars().any(|c| c < ' ' || "<>\"|?*".contains(c))
            || matches!(stem.as_str(), "CON" | "PRN" | "AUX" | "NUL")
            || (stem.len() == 4
                && (stem.starts_with("COM") || stem.starts_with("LPT"))
                && matches!(stem.as_bytes()[3], b'1'..=b'9'))
        {
            bail!("unsafe or non-portable Git path: {path:?}");
        }
    }
    Ok(())
}

pub fn validate_files(files: &Files) -> Result<()> {
    let mut names = BTreeSet::new();
    let mut directories = BTreeMap::new();
    for path in files.keys() {
        validate_path(path)?;
        let mut parent = path.as_str();
        while let Some((p, _)) = parent.rsplit_once('/') {
            if directories
                .insert(p.to_lowercase(), p)
                .is_some_and(|old| old != p)
            {
                bail!("case-insensitive directory collision: {path}");
            }
            parent = p;
        }
        // A common policy on all platforms prevents surprises when another device is Windows.
        if !names.insert(path.to_lowercase()) {
            bail!("case-insensitive filename collision: {path}");
        }
    }
    for path in &names {
        let mut parent = path.as_str();
        while let Some((p, _)) = parent.rsplit_once('/') {
            if names.contains(p) {
                bail!("file/directory collision: {path}");
            }
            parent = p;
        }
    }
    Ok(())
}

pub struct ReconcileOptions<'a> {
    pub policy: ConflictPolicy,
    pub device: &'a str,
    pub stamp: &'a str,
    pub text_extensions: &'a [String],
}

pub fn reconcile(
    repo: &Repository,
    base: &Files,
    local: &Files,
    remote: &Files,
    options: ReconcileOptions<'_>,
) -> Result<Reconciliation> {
    let ReconcileOptions {
        policy,
        device,
        stamp,
        text_extensions,
    } = options;
    let paths: BTreeSet<_> = base
        .keys()
        .chain(local.keys())
        .chain(remote.keys())
        .cloned()
        .collect();
    let mut result = Files::new();
    let mut conflicts = Vec::new();
    let mut reserved: BTreeSet<_> = paths.iter().map(|p| p.to_lowercase()).collect();
    for path in paths {
        let b = base.get(&path).copied();
        let l = local.get(&path).copied();
        let r = remote.get(&path).copied();
        let chosen = if l == r || r == b {
            l
        } else if l == b {
            r
        } else if let (Some(lv), Some(rv)) = (l, r) {
            if let Some(merged) = merge_text(repo, &path, b, lv, rv, text_extensions)? {
                Some(merged)
            } else {
                let mut conflict = Conflict {
                    path: path.clone(),
                    local: lv.oid.to_string(),
                    remote: rv.oid.to_string(),
                    copy: None,
                };
                let chosen = match policy {
                    ConflictPolicy::PreferLocal | ConflictPolicy::Ask => lv,
                    ConflictPolicy::PreferRemote => rv,
                    ConflictPolicy::KeepBoth => {
                        let copy = conflict_name(&path, device, stamp, &mut reserved);
                        result.insert(copy.clone(), lv);
                        conflict.copy = Some(copy);
                        rv
                    }
                };
                conflicts.push(conflict);
                Some(chosen)
            }
        } else {
            // Both diverged from base, and one was deleted: keep the modification.
            l.or(r)
        };
        if let Some(value) = chosen {
            result.insert(path, value);
        }
    }
    validate_files(&result)?;
    Ok(Reconciliation {
        files: result,
        conflicts,
    })
}

fn merge_text(
    repo: &Repository,
    path: &str,
    base: Option<Version>,
    local: Version,
    remote: Version,
    extensions: &[String],
) -> Result<Option<Version>> {
    let ext = path.rsplit_once('.').map(|(_, ext)| ext).unwrap_or("");
    if !extensions.iter().any(|e| e.eq_ignore_ascii_case(ext)) || local.mode != remote.mode {
        return Ok(None);
    }
    let b = base.map(|v| repo.find_blob(v.oid)).transpose()?;
    let l = repo.find_blob(local.oid)?;
    let r = repo.find_blob(remote.oid)?;
    let bytes = [
        b.as_ref().map(|b| b.content()).unwrap_or(b""),
        l.content(),
        r.content(),
    ];
    if bytes
        .iter()
        .any(|b| b.contains(&0) || std::str::from_utf8(b).is_err())
    {
        return Ok(None);
    }
    let entry = |oid, mode| IndexEntry {
        ctime: IndexTime::new(0, 0),
        mtime: IndexTime::new(0, 0),
        dev: 0,
        ino: 0,
        mode,
        uid: 0,
        gid: 0,
        file_size: 0,
        id: oid,
        flags: 0,
        flags_extended: 0,
        path: path.as_bytes().to_vec(),
    };
    let ancestor = entry(base.map(|v| v.oid).unwrap_or(repo.blob(b"")?), local.mode);
    let ours = entry(local.oid, local.mode);
    let theirs = entry(remote.oid, remote.mode);
    let mut options = MergeFileOptions::new();
    options.favor(FileFavor::Normal);
    let merged = repo.merge_file_from_index(&ancestor, &ours, &theirs, Some(&mut options))?;
    if !merged.is_automergeable() {
        return Ok(None);
    }
    Ok(Some(Version {
        oid: repo.blob(merged.content())?,
        mode: local.mode,
    }))
}

fn conflict_name(path: &str, device: &str, stamp: &str, reserved: &mut BTreeSet<String>) -> String {
    let clean: String = device
        .chars()
        .map(|c| {
            if c.is_control() || "<>:\"/\\|?*".contains(c) {
                '_'
            } else {
                c
            }
        })
        .take(40)
        .collect();
    let (dir, file) = path
        .rsplit_once('/')
        .map(|(d, f)| (format!("{d}/"), f))
        .unwrap_or((String::new(), path));
    let (stem, ext) = file
        .rsplit_once('.')
        .filter(|(s, _)| !s.is_empty())
        .map(|(s, e)| (s, format!(".{e}")))
        .unwrap_or((file, String::new()));
    for n in 0.. {
        let suffix = if n == 0 {
            String::new()
        } else {
            format!(" {n}")
        };
        let name = format!("{dir}{stem} (冲突 · {clean} · {stamp}{suffix}){ext}");
        if reserved.insert(name.to_lowercase()) {
            return name;
        }
    }
    unreachable!()
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn refuses_unsafe_or_cross_platform_collisions() {
        for path in [
            "../file",
            "/absolute",
            ".git/config",
            "a/.GIT/HEAD",
            "C:/x",
            "a\\b",
            "CON.txt",
            "a.",
            "NUL",
            "COM1.txt",
        ] {
            assert!(validate_path(path).is_err(), "{path}");
        }
        let v = Version {
            oid: Oid::ZERO_SHA1,
            mode: 0o100644,
        };
        let files = Files::from([("A.txt".into(), v), ("a.txt".into(), v)]);
        assert!(validate_files(&files).is_err());
        let files = Files::from([("a".into(), v), ("a/b.txt".into(), v)]);
        assert!(validate_files(&files).is_err());
    }
    #[test]
    fn conflict_copy_never_replaces_a_preexisting_name() {
        let mut names = BTreeSet::new();
        let a = conflict_name("dir/中文.md", "device", "10-18 14.30", &mut names);
        let b = conflict_name("dir/中文.md", "device", "10-18 14.30", &mut names);
        assert_ne!(a, b);
        assert!(b.ends_with(" 1).md"));
    }
}
