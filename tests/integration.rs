//! End-to-end tests against a local `file://` blobless remote and a
//! superproject that pins it. Mirrors PLAN.md's verification scenarios.

use std::path::{Path, PathBuf};
use std::process::{Command, Output};
use std::sync::atomic::{AtomicU32, Ordering};

const BIN: &str = env!("CARGO_BIN_EXE_picky");

/// A throwaway directory under the system temp dir, removed on drop.
struct TmpDir(PathBuf);

impl TmpDir {
    fn new() -> Self {
        static COUNTER: AtomicU32 = AtomicU32::new(0);
        let n = COUNTER.fetch_add(1, Ordering::Relaxed);
        let nanos = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap()
            .as_nanos();
        let dir = std::env::temp_dir().join(format!("picky-it-{}-{n}-{nanos}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        TmpDir(dir)
    }
    fn path(&self) -> &Path {
        &self.0
    }
}

impl Drop for TmpDir {
    fn drop(&mut self) {
        let _ = std::fs::remove_dir_all(&self.0);
    }
}

fn git(dir: &Path, args: &[&str]) -> String {
    let out = Command::new("git")
        .arg("-C")
        .arg(dir)
        .args(args)
        .env("GIT_AUTHOR_NAME", "t")
        .env("GIT_AUTHOR_EMAIL", "t@t")
        .env("GIT_COMMITTER_NAME", "t")
        .env("GIT_COMMITTER_EMAIL", "t@t")
        .output()
        .unwrap();
    assert!(
        out.status.success(),
        "git {args:?} failed: {}",
        String::from_utf8_lossy(&out.stderr)
    );
    String::from_utf8_lossy(&out.stdout).trim_end().to_string()
}

fn picky(dir: &Path, args: &[&str]) -> Output {
    Command::new(BIN)
        .arg("--quiet")
        .args(args)
        .current_dir(dir)
        .env("GIT_AUTHOR_NAME", "t")
        .env("GIT_AUTHOR_EMAIL", "t@t")
        .env("GIT_COMMITTER_NAME", "t")
        .env("GIT_COMMITTER_EMAIL", "t@t")
        .env("XDG_CACHE_HOME", dir.join(".cache")) // isolate the ref cache
        .output()
        .unwrap()
}

/// Drive a dynamic-completion request and return the non-flag candidate lines.
fn complete(dir: &Path, index: usize, words: &[&str]) -> Vec<String> {
    let out = Command::new(BIN)
        .env("COMPLETE", "bash")
        .env("_CLAP_COMPLETE_INDEX", index.to_string())
        .env("XDG_CACHE_HOME", dir.join(".cache"))
        .arg("--")
        .args(words)
        .current_dir(dir)
        .output()
        .unwrap();
    String::from_utf8_lossy(&out.stdout)
        .lines()
        .filter(|l| !l.is_empty() && !l.starts_with("--"))
        .map(str::to_string)
        .collect()
}

fn write(path: &Path, contents: &str) {
    if let Some(parent) = path.parent() {
        std::fs::create_dir_all(parent).unwrap();
    }
    std::fs::write(path, contents).unwrap();
}

/// Build an upstream repo (two commits, tags v1/v2), a bare filter-allowing
/// remote, and a superproject pinning `ext/dep` at v1 with sparse `/keep/` +
/// `/src/`. Returns (tmp, super_dir, v1_sha).
fn fixture(sparse: &[&str], patches: Option<&str>) -> (TmpDir, PathBuf, String) {
    let tmp = TmpDir::new();
    let root = tmp.path().to_path_buf();

    let up = root.join("up");
    std::fs::create_dir_all(&up).unwrap();
    git(&up, &["init", "-q"]);
    write(&up.join("keep/a.txt"), "keep\n");
    write(&up.join("drop/b.txt"), "drop\n");
    write(&up.join("src/c.txt"), "line1\nline2\nline3\n");
    write(&up.join("README"), "root\n");
    git(&up, &["add", "-A"]);
    git(&up, &["commit", "-qm", "v1"]);
    git(&up, &["tag", "v1"]);
    let v1 = git(&up, &["rev-parse", "HEAD"]);
    write(&up.join("src/c.txt"), "line1\nline2\nline3\nline4\n");
    git(&up, &["add", "-A"]);
    git(&up, &["commit", "-qm", "v2"]);
    git(&up, &["tag", "v2"]);

    let remote = root.join("remote.git");
    let st = Command::new("git")
        .args(["clone", "-q", "--bare"])
        .arg(&up)
        .arg(&remote)
        .status()
        .unwrap();
    assert!(st.success());
    git(&remote, &["config", "uploadpack.allowFilter", "true"]);
    git(
        &remote,
        &["config", "uploadpack.allowanysha1inwant", "true"],
    );

    let sup = root.join("super");
    std::fs::create_dir_all(&sup).unwrap();
    git(&sup, &["init", "-q"]);
    git(
        &sup,
        &[
            "update-index",
            "--add",
            "--cacheinfo",
            &format!("160000,{v1},ext/dep"),
        ],
    );
    let url = format!("file://{}", remote.display());
    git(
        &sup,
        &[
            "config",
            "-f",
            ".gitmodules",
            "submodule.ext/dep.path",
            "ext/dep",
        ],
    );
    git(
        &sup,
        &["config", "-f", ".gitmodules", "submodule.ext/dep.url", &url],
    );
    git(
        &sup,
        &[
            "config",
            "-f",
            ".gitmodules",
            "submodule.ext/dep.shallow",
            "true",
        ],
    );
    for pat in sparse {
        git(
            &sup,
            &[
                "config",
                "-f",
                ".gitmodules",
                "--add",
                "picky.ext/dep.sparse",
                pat,
            ],
        );
    }
    if let Some(p) = patches {
        git(
            &sup,
            &["config", "-f", ".gitmodules", "picky.ext/dep.patches", p],
        );
    }
    git(&sup, &["add", ".gitmodules"]);
    git(&sup, &["commit", "-qm", "sub"]);

    (tmp, sup, v1)
}

#[test]
fn init_is_sparse_shallow_and_blobless() {
    let (_tmp, sup, _v1) = fixture(&["/keep/", "/src/"], None);
    let dep = sup.join("ext/dep");

    let out = picky(&sup, &["init", "ext/dep"]);
    assert!(
        out.status.success(),
        "{}",
        String::from_utf8_lossy(&out.stderr)
    );

    // Only the sparse paths are materialized.
    assert!(dep.join("keep").is_dir());
    assert!(dep.join("src").is_dir());
    assert!(!dep.join("drop").exists());

    // Shallow clone.
    assert_eq!(git(&dep, &["rev-parse", "--is-shallow-repository"]), "true");

    // Blobless: out-of-sparse blobs (drop/, README) were never fetched.
    let missing = git(&dep, &["rev-list", "--objects", "--all", "--missing=print"]);
    let missing_count = missing.lines().filter(|l| l.starts_with('?')).count();
    assert!(
        missing_count >= 2,
        "expected missing blobs, got:\n{missing}"
    );
}

#[test]
fn init_is_idempotent() {
    let (_tmp, sup, _v1) = fixture(&["/src/"], None);
    assert!(picky(&sup, &["init", "ext/dep"]).status.success());
    let second = picky(&sup, &["init", "ext/dep"]);
    assert!(
        second.status.success(),
        "{}",
        String::from_utf8_lossy(&second.stderr)
    );
    assert!(sup.join("ext/dep/src").is_dir());
}

#[test]
fn update_moves_the_pin() {
    let (_tmp, sup, v1) = fixture(&["/src/"], None);
    let dep = sup.join("ext/dep");
    assert!(picky(&sup, &["init", "ext/dep"]).status.success());
    assert_eq!(git(&dep, &["rev-parse", "HEAD"]), v1);

    let out = picky(&sup, &["update", "ext/dep", "v2"]);
    assert!(
        out.status.success(),
        "{}",
        String::from_utf8_lossy(&out.stderr)
    );
    assert_ne!(git(&dep, &["rev-parse", "HEAD"]), v1);
    // src/c.txt at v2 has the extra line.
    assert!(
        std::fs::read_to_string(dep.join("src/c.txt"))
            .unwrap()
            .contains("line4")
    );
}

#[test]
fn patch_stack_applies_and_broken_patch_is_fatal() {
    let (_tmp, sup, _v1) = fixture(&["/src/"], Some("patches"));
    let dep = sup.join("ext/dep");
    assert!(picky(&sup, &["init", "ext/dep"]).status.success());

    // A good patch applies cleanly during a refresh.
    write(
        &sup.join("patches/0001-good.patch"),
        "--- a/src/c.txt\n+++ b/src/c.txt\n@@ -1,3 +1,3 @@\n line1\n-line2\n+line2-patched\n line3\n",
    );
    let out = picky(&sup, &["update"]);
    assert!(
        out.status.success(),
        "{}",
        String::from_utf8_lossy(&out.stderr)
    );
    assert!(
        std::fs::read_to_string(dep.join("src/c.txt"))
            .unwrap()
            .contains("line2-patched")
    );

    // A broken patch makes the run fail fatally.
    write(
        &sup.join("patches/0002-bad.patch"),
        "--- a/src/c.txt\n+++ b/src/c.txt\n@@ -1,3 +1,3 @@\n nope\n-wrong\n+x\n bad\n",
    );
    let out = picky(&sup, &["update"]);
    assert!(!out.status.success(), "broken patch should fail");
}

#[test]
fn sparse_subcommand_widens_and_narrows() {
    let (_tmp, sup, _v1) = fixture(&["/src/"], None);
    let dep = sup.join("ext/dep");
    assert!(picky(&sup, &["init", "ext/dep"]).status.success());
    assert!(dep.join("src").is_dir());
    assert!(!dep.join("keep").exists());

    // Widen: add /keep/ and reconcile.
    let out = picky(&sup, &["sparse", "ext/dep", "--add", "/keep/"]);
    assert!(
        out.status.success(),
        "{}",
        String::from_utf8_lossy(&out.stderr)
    );
    assert!(dep.join("keep").is_dir(), "added path should materialize");
    assert!(dep.join("src").is_dir());
    // Persisted to .gitmodules.
    let cfg = git(
        &sup,
        &[
            "config",
            "-f",
            ".gitmodules",
            "--get-all",
            "picky.ext/dep.sparse",
        ],
    );
    assert!(cfg.lines().any(|l| l == "/keep/"));

    // Narrow: remove /src/ and reconcile.
    let out = picky(&sup, &["sparse", "ext/dep", "--remove", "/src/"]);
    assert!(
        out.status.success(),
        "{}",
        String::from_utf8_lossy(&out.stderr)
    );
    assert!(!dep.join("src").exists(), "removed path should be trimmed");
    assert!(dep.join("keep").is_dir());
}

#[test]
fn update_ref_completion_lists_remote_refs() {
    let (_tmp, sup, _v1) = fixture(&["/src/"], None);
    // Populate the ref cache from the (file://) remote.
    let out = picky(&sup, &["refresh", "ext/dep"]);
    assert!(
        out.status.success(),
        "{}",
        String::from_utf8_lossy(&out.stderr)
    );

    // Completing the ref slot: words = [picky, update, ext/dep, ""], index 3.
    let refs = complete(&sup, 3, &["picky", "update", "ext/dep", ""]);
    assert!(refs.iter().any(|r| r == "v1"), "expected v1 in {refs:?}");
    assert!(refs.iter().any(|r| r == "v2"), "expected v2 in {refs:?}");

    // Prefix filtering narrows to the matching tag.
    let only_v2 = complete(&sup, 3, &["picky", "update", "ext/dep", "v2"]);
    assert!(only_v2.iter().any(|r| r == "v2"));
    assert!(!only_v2.iter().any(|r| r == "v1"));
}
