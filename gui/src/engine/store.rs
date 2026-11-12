//! The version store: a plain git repository, driven through libgit2.
//!
//! Deliberately not a new history format. A folder of contracts that is also a
//! git repo can be backed up with `git bundle`, inspected with `git log`, and
//! recovered with `git fsck` — none of which is true of a bespoke snapshot
//! database. Snapshots are whole files because Word documents cannot be
//! merged, so storing deltas would buy nothing and cost correctness.

use std::path::{Path, PathBuf};

use git2::{Commit, Oid, Repository, Sort};

use super::text;

#[derive(Clone, Debug)]
pub struct Snapshot {
    pub id: Oid,
    pub short: String,
    pub when: i64,
    pub subject: String,
    pub author: String,
}

impl Snapshot {
    /// Local wall-clock time, formatted without pulling in a date crate.
    pub fn when_text(&self) -> String {
        let (y, mo, d, h, mi) = civil(self.when);
        format!("{y:04}-{mo:02}-{d:02} {h:02}:{mi:02}")
    }
}

/// Days-since-epoch to a civil date (Howard Hinnant's algorithm).
fn civil(secs: i64) -> (i64, u32, u32, u32, u32) {
    let days = secs.div_euclid(86_400);
    let rem = secs.rem_euclid(86_400);
    let z = days + 719_468;
    let era = z.div_euclid(146_097);
    let doe = z.rem_euclid(146_097);
    let yoe = (doe - doe / 1460 + doe / 36_524 - doe / 146_096) / 365;
    let y = yoe + era * 400;
    let doy = doe - (365 * yoe + yoe / 4 - yoe / 100);
    let mp = (5 * doy + 2) / 153;
    let d = (doy - (153 * mp + 2) / 5 + 1) as u32;
    let m = if mp < 10 { mp + 3 } else { mp - 9 } as u32;
    let y = if m <= 2 { y + 1 } else { y };
    (y, m, d, (rem / 3600) as u32, ((rem % 3600) / 60) as u32)
}

pub struct Store {
    repo: Repository,
    root: PathBuf,
}

impl Store {
    /// Open the repository containing `path`, or `None` if there is none.
    pub fn discover(path: &Path) -> Option<Store> {
        let repo = Repository::discover(path).ok()?;
        let root = repo.workdir()?.to_path_buf();
        Some(Store { repo, root })
    }

    pub fn init(path: &Path) -> Result<Store, String> {
        std::fs::create_dir_all(path).map_err(|e| e.to_string())?;
        let repo = Repository::init(path).map_err(|e| e.to_string())?;
        let store = Store { repo, root: path.to_path_buf() };
        store.configure()?;
        Ok(store)
    }

    pub fn root(&self) -> &Path {
        &self.root
    }

    /// Repo-local settings, so `git diff` on a docx works inside this store and
    /// nowhere else. `core.quotepath=false` is not cosmetic: without it git
    /// escapes non-ASCII paths and every later lookup by name misses.
    pub fn configure(&self) -> Result<(), String> {
        let mut cfg = self.repo.config().map_err(|e| e.to_string())?;
        cfg.set_str("core.quotepath", "false").map_err(|e| e.to_string())?;
        if cfg.get_string("user.email").is_err() {
            cfg.set_str("user.email", "docver@localhost").map_err(|e| e.to_string())?;
            let name = std::env::var("USER").unwrap_or_else(|_| "docver".into());
            cfg.set_str("user.name", &name).map_err(|e| e.to_string())?;
        }
        let attrs = self.root.join(".gitattributes");
        let want = "*.docx diff=docx\n*.odt diff=odt\n*.rtf diff=rtf\n";
        if std::fs::read_to_string(&attrs).unwrap_or_default() != want {
            std::fs::write(&attrs, want).map_err(|e| e.to_string())?;
        }
        let ignore = self.root.join(".gitignore");
        if !ignore.exists() {
            // Word's ~$ lock files and LibreOffice's .~lock files are not
            // documents; committing them would put noise in every snapshot.
            std::fs::write(&ignore, "~$*\n.~lock.*#\n*.tmp\n").map_err(|e| e.to_string())?;
        }
        Ok(())
    }

    /// Documents in the working tree, relative to the root, sorted.
    pub fn documents(&self) -> Vec<String> {
        let mut out = Vec::new();
        let walker = walkdir(&self.root, 0);
        for path in walker {
            let rel = match path.strip_prefix(&self.root) {
                Ok(r) => r.to_string_lossy().replace('\\', "/"),
                Err(_) => continue,
            };
            if text::is_document(&rel) {
                out.push(rel);
            }
        }
        out.sort();
        out
    }

    /// Snapshots that touched `path`, newest first.
    pub fn history(&self, path: &str) -> Vec<Snapshot> {
        let mut out = Vec::new();
        let mut walk = match self.repo.revwalk() {
            Ok(w) => w,
            Err(_) => return out,
        };
        if walk.push_head().is_err() {
            return out;
        }
        let _ = walk.set_sorting(Sort::TIME);
        let commits: Vec<Commit> = walk.flatten().filter_map(|o| self.repo.find_commit(o).ok()).collect();
        let blob_of = |c: &Commit| -> Option<Oid> {
            c.tree().ok().and_then(|t| t.get_path(Path::new(path)).ok()).map(|e| e.id())
        };
        // A commit belongs to this document's history when its blob differs
        // from the blob in the next-older commit — that is the commit that
        // introduced the change. Merely *containing* the file is not enough:
        // every later commit in the repo would qualify, and the version list
        // would fill with snapshots of other documents.
        for (i, c) in commits.iter().enumerate() {
            let here = blob_of(c);
            if here.is_none() {
                continue;
            }
            let older = commits.get(i + 1).and_then(|n| blob_of(n));
            if here == older {
                continue;
            }
            let sig = c.author();
            out.push(Snapshot {
                id: c.id(),
                short: short_id(c.id()),
                when: sig.when().seconds(),
                subject: c.summary().ok().flatten().unwrap_or("(no message)").to_owned(),
                author: sig.name().unwrap_or("").to_string(),
            });
        }
        out
    }

    pub fn blob_at(&self, rev: Oid, path: &str) -> Option<Vec<u8>> {
        let commit = self.repo.find_commit(rev).ok()?;
        let entry = commit.tree().ok()?.get_path(Path::new(path)).ok()?;
        let blob = self.repo.find_blob(entry.id()).ok()?;
        Some(blob.content().to_vec())
    }

    pub fn worktree_bytes(&self, path: &str) -> Option<Vec<u8>> {
        std::fs::read(self.root.join(path)).ok()
    }

    pub fn worktree_exists(&self, path: &str) -> bool {
        self.root.join(path).exists()
    }

    /// Commit the current contents of `path`.
    pub fn save(&self, path: &str, message: &str) -> Result<Oid, String> {
        let mut index = self.repo.index().map_err(|e| e.to_string())?;
        index.add_path(Path::new(path)).map_err(|e| e.to_string())?;
        // The store's own configuration belongs in the store: a repo whose
        // `git status` always shows two untracked files looks broken, and a
        // clone of it would silently lose the textconv attributes.
        for cfg in [".gitattributes", ".gitignore"] {
            if self.root.join(cfg).exists() {
                let _ = index.add_path(Path::new(cfg));
            }
        }
        index.write().map_err(|e| e.to_string())?;
        let tree_id = index.write_tree().map_err(|e| e.to_string())?;
        let tree = self.repo.find_tree(tree_id).map_err(|e| e.to_string())?;

        let sig = self
            .repo
            .signature()
            .or_else(|_| git2::Signature::now("docver", "docver@localhost"))
            .map_err(|e| e.to_string())?;

        let parent = self.repo.head().ok().and_then(|h| h.peel_to_commit().ok());
        let parents: Vec<&git2::Commit> = parent.iter().collect();
        let msg = if message.trim().is_empty() { "snapshot" } else { message.trim() };
        self.repo
            .commit(Some("HEAD"), &sig, &sig, msg, &tree, &parents)
            .map_err(|e| e.to_string())
    }

    /// Write `rev`'s copy of `path` beside the working file, never over it.
    ///
    /// Rollback must not be the one operation that can destroy work, so the
    /// restored file gets a suffix and the user decides what to do with it.
    pub fn restore(&self, rev: Oid, path: &str) -> Result<PathBuf, String> {
        let bytes = self.blob_at(rev, path).ok_or("that version does not contain this file")?;
        let target = self.root.join(path);
        let stem = target.file_stem().map(|s| s.to_string_lossy().into_owned()).unwrap_or_default();
        let ext = target.extension().map(|s| s.to_string_lossy().into_owned());
        let mut name = format!("{stem}.{}.", short_id(rev));
        if let Some(e) = &ext {
            name.push_str(e);
        }
        let mut out = target.with_file_name(name);
        let mut n = 2;
        while out.exists() {
            let mut name = format!("{stem}.{}.{n}.", short_id(rev));
            if let Some(e) = &ext {
                name.push_str(e);
            }
            out = target.with_file_name(name);
            n += 1;
        }
        std::fs::write(&out, bytes).map_err(|e| e.to_string())?;
        Ok(out)
    }

}

fn short_id(oid: Oid) -> String {
    oid.to_string()[..7].to_string()
}

/// Shallow walk: documents live at the top of a store, not in a source tree.
fn walkdir(root: &Path, depth: usize) -> Vec<PathBuf> {
    let mut out = Vec::new();
    if depth > 3 {
        return out;
    }
    let entries = match std::fs::read_dir(root) {
        Ok(e) => e,
        Err(_) => return out,
    };
    for entry in entries.flatten() {
        let path = entry.path();
        let name = entry.file_name().to_string_lossy().into_owned();
        if name.starts_with('.') || name.starts_with("~$") {
            continue;
        }
        match entry.file_type() {
            Ok(t) if t.is_dir() => out.extend(walkdir(&path, depth + 1)),
            Ok(t) if t.is_file() => out.push(path),
            _ => {}
        }
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn civil_epoch() {
        assert_eq!(civil(0), (1970, 1, 1, 0, 0));
        assert_eq!(civil(1_700_000_000), (2023, 11, 14, 22, 13));
    }

    #[test]
    fn save_and_history_roundtrip() {
        let dir = std::env::temp_dir().join(format!("docver-test-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        let store = Store::init(&dir).unwrap();
        std::fs::write(dir.join("a.txt"), "one\n").unwrap();
        store.save("a.txt", "first").unwrap();
        std::fs::write(dir.join("a.txt"), "two\n").unwrap();
        store.save("a.txt", "second").unwrap();

        let hist = store.history("a.txt");
        assert_eq!(hist.len(), 2);
        assert_eq!(hist[0].subject, "second");
        assert_eq!(hist[1].subject, "first");

        let old = store.blob_at(hist[1].id, "a.txt").unwrap();
        assert_eq!(old, b"one\n");

        let restored = store.restore(hist[1].id, "a.txt").unwrap();
        assert_eq!(std::fs::read(&restored).unwrap(), b"one\n");
        // The working file is untouched.
        assert_eq!(std::fs::read(dir.join("a.txt")).unwrap(), b"two\n");
        let _ = std::fs::remove_dir_all(&dir);
    }
}
