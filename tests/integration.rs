//! End-to-end tests against a local `file://` blobless remote and a
//! superproject that pins it, covering the workflows in QRH.md.

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
    picky_with_env(dir, args, &[])
}

/// Like [`picky`], but without `--quiet`, for asserting on the success and
/// progress lines `--quiet` suppresses.
fn picky_loud(dir: &Path, args: &[&str]) -> Output {
    let mut cmd = Command::new(BIN);
    cmd.args(args)
        .current_dir(dir)
        .env("GIT_AUTHOR_NAME", "t")
        .env("GIT_AUTHOR_EMAIL", "t@t")
        .env("GIT_COMMITTER_NAME", "t")
        .env("GIT_COMMITTER_EMAIL", "t@t")
        .env("XDG_CACHE_HOME", dir.join(".cache"));
    cmd.output().unwrap()
}

/// Like [`picky`], with extra environment variables set on the child (e.g.
/// `PICKY_TRUST_HOOKS`).
fn picky_with_env(dir: &Path, args: &[&str], envs: &[(&str, &str)]) -> Output {
    let mut cmd = Command::new(BIN);
    cmd.arg("--quiet")
        .args(args)
        .current_dir(dir)
        .env("GIT_AUTHOR_NAME", "t")
        .env("GIT_AUTHOR_EMAIL", "t@t")
        .env("GIT_COMMITTER_NAME", "t")
        .env("GIT_COMMITTER_EMAIL", "t@t")
        .env("XDG_CACHE_HOME", dir.join(".cache")); // isolate the ref cache
    for (k, v) in envs {
        cmd.env(k, v);
    }
    cmd.output().unwrap()
}

/// Like [`picky`], but feeds `input` to the child's stdin (for `--stdin`).
fn picky_stdin(dir: &Path, args: &[&str], input: &str) -> Output {
    use std::io::Write;
    use std::process::Stdio;
    let mut child = Command::new(BIN)
        .arg("--quiet")
        .args(args)
        .current_dir(dir)
        .env("XDG_CACHE_HOME", dir.join(".cache"))
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()
        .unwrap();
    child
        .stdin
        .take()
        .unwrap()
        .write_all(input.as_bytes())
        .unwrap();
    child.wait_with_output().unwrap()
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

/// Build an upstream repo (two commits, tags v1/v2) and a bare local
/// `file://` remote allowing partial clones + arbitrary-SHA fetches. Returns
/// (tmp, remote `file://` URL, v1 SHA). Shared by [`fixture`] (which also
/// declares + commits a submodule pointing at it) and tests that exercise the
/// declaration step themselves (`add`, `remove`).
fn remote_fixture() -> (TmpDir, String, String) {
    let tmp = TmpDir::new();
    let root = tmp.path().to_path_buf();

    let up = root.join("up");
    std::fs::create_dir_all(&up).unwrap();
    // Pin the default branch name explicitly: tests reference it by name
    // and shouldn't depend on the runner's `init.defaultBranch`.
    git(&up, &["init", "-q", "-b", "main"]);
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

    let url = format!("file://{}", remote.display());
    (tmp, url, v1)
}

/// [`remote_fixture`] plus a superproject that already declares + commits
/// `ext/dep` pinned at v1 with the given sparse patterns. Returns (tmp,
/// super_dir, v1_sha).
fn fixture(sparse: &[&str], patches: Option<&str>) -> (TmpDir, PathBuf, String) {
    let (tmp, url, v1) = remote_fixture();
    let root = tmp.path().to_path_buf();

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

/// A fresh, empty superproject under `remote_fixture`'s tmp root; nothing
/// declared yet, for tests that exercise `add`/`remove` themselves.
fn empty_super(tmp: &TmpDir) -> PathBuf {
    let sup = tmp.path().join("super");
    std::fs::create_dir_all(&sup).unwrap();
    git(&sup, &["init", "-q"]);
    sup
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
fn init_rebuilds_after_gitdir_deleted() {
    let (_tmp, sup, _v1) = fixture(&["/src/"], None);
    let dep = sup.join("ext/dep");
    assert!(picky(&sup, &["init", "ext/dep"]).status.success());
    assert!(dep.join("src").is_dir());

    // Delete ONLY the submodule git dir, leaving the now-dangling `.git`
    // gitlink in the worktree (a common "let me start the submodule over" move).
    let gitdir = sup.join(".git/modules/ext/dep");
    assert!(gitdir.is_dir());
    std::fs::remove_dir_all(&gitdir).unwrap();
    assert!(dep.join(".git").exists(), "dangling gitlink should remain");

    // Re-init must rebuild the git dir, not fail on `remote add`.
    let out = picky(&sup, &["init", "ext/dep"]);
    assert!(
        out.status.success(),
        "{}",
        String::from_utf8_lossy(&out.stderr)
    );
    assert!(gitdir.is_dir(), "git dir should be rebuilt");
    assert!(dep.join("src").is_dir(), "sparse checkout restored");
}

#[test]
fn add_declares_and_checks_out_a_new_submodule() {
    let (tmp, url, v1) = remote_fixture();
    let sup = empty_super(&tmp);

    let out = picky(
        &sup,
        &["add", &url, "ext/dep", "--sparse", "/src/", "--ref", "v1"],
    );
    assert!(
        out.status.success(),
        "{}",
        String::from_utf8_lossy(&out.stderr)
    );

    // Sparse checkout materialized at the requested ref.
    assert!(sup.join("ext/dep/src").is_dir());
    assert!(
        !sup.join("ext/dep/drop").exists(),
        "unlisted path should not materialize"
    );
    assert_eq!(git(&sup.join("ext/dep"), &["rev-parse", "HEAD"]), v1);

    // .gitmodules written and both it and the gitlink staged.
    let staged = git(&sup, &["diff", "--cached", "--name-only"]);
    assert!(
        staged.lines().any(|l| l == ".gitmodules"),
        "staged: {staged}"
    );
    assert!(staged.lines().any(|l| l == "ext/dep"), "staged: {staged}");
    let cfg_url = git(
        &sup,
        &[
            "config",
            "-f",
            ".gitmodules",
            "--get",
            "submodule.ext/dep.url",
        ],
    );
    assert_eq!(cfg_url, url);
}

#[test]
fn add_failure_leaves_no_partial_gitmodules_state() {
    let (tmp, url, _v1) = remote_fixture();
    let sup = empty_super(&tmp);

    // A ref that doesn't exist on the remote makes the fetch fail.
    let out = picky(&sup, &["add", &url, "ext/dep", "--ref", "does-not-exist"]);
    assert!(!out.status.success(), "add with a bad ref should fail");

    // `.gitmodules` must never have been written, and nothing staged.
    assert!(
        !sup.join(".gitmodules").exists(),
        ".gitmodules should not exist after a failed add"
    );
    let staged = git(&sup, &["diff", "--cached", "--name-only"]);
    assert!(
        staged.is_empty(),
        "nothing should be staged after a failed add, got: {staged}"
    );
}

/// A path that escapes the superproject must be refused up front. It used to
/// fail only at the final `git add`, by which point the clone was materialized
/// outside the repo, the post-update hook had run there, and a staged
/// `.gitmodules` entry was left behind.
#[test]
fn add_refuses_a_path_escaping_the_superproject() {
    let (tmp, url, _v1) = remote_fixture();
    let sup = empty_super(&tmp);
    let outside = tmp.path().join("outside");
    std::fs::create_dir_all(&outside).unwrap();

    let out = picky(&sup, &["add", &url, "../outside/escaped"]);
    assert!(!out.status.success(), "escaping path should be refused");
    assert!(
        String::from_utf8_lossy(&out.stderr).contains("must stay inside the superproject"),
        "stderr: {}",
        String::from_utf8_lossy(&out.stderr)
    );
    assert!(
        !outside.join("escaped").exists(),
        "nothing may be created outside the superproject"
    );
    assert!(
        !sup.join(".gitmodules").exists(),
        "no declaration may be written"
    );
}

/// The same guard on the destructive side: a hostile `.gitmodules` must not
/// aim `remove`'s recursive delete outside the repo.
#[test]
fn remove_refuses_a_path_escaping_the_superproject() {
    let (tmp, url, _v1) = remote_fixture();
    let sup = empty_super(&tmp);
    git(
        &sup,
        &["config", "-f", ".gitmodules", "submodule.evil.path", ".."],
    );
    git(
        &sup,
        &["config", "-f", ".gitmodules", "submodule.evil.url", &url],
    );

    let out = picky(&sup, &["remove", "..", "--yes"]);
    assert!(!out.status.success(), "escaping path should be refused");
    assert!(
        String::from_utf8_lossy(&out.stderr).contains("must stay inside the superproject"),
        "stderr: {}",
        String::from_utf8_lossy(&out.stderr)
    );
    assert!(
        sup.exists() && tmp.path().exists(),
        "nothing may be deleted"
    );
}

#[test]
fn add_rerun_clears_stale_optional_keys() {
    let (tmp, url, _v1) = remote_fixture();
    let sup = empty_super(&tmp);

    let out = picky(
        &sup,
        &["add", &url, "ext/dep", "--branch", "main", "--depth", "1"],
    );
    assert!(
        out.status.success(),
        "{}",
        String::from_utf8_lossy(&out.stderr)
    );
    let cfg = git(&sup, &["config", "-f", ".gitmodules", "--list"]);
    assert!(cfg.contains("submodule.ext/dep.branch=main"), "{cfg}");
    assert!(cfg.contains("picky.ext/dep.depth=1"), "{cfg}");

    // Re-add the same path without --branch/--depth: the declaration must
    // converge to exactly the new args, not keep the old values around.
    let out = picky(&sup, &["add", &url, "ext/dep"]);
    assert!(
        out.status.success(),
        "{}",
        String::from_utf8_lossy(&out.stderr)
    );
    let cfg = git(&sup, &["config", "-f", ".gitmodules", "--list"]);
    assert!(
        !cfg.contains("branch"),
        "stale branch should have been cleared, got: {cfg}"
    );
    assert!(
        !cfg.contains("depth"),
        "stale depth should have been cleared, got: {cfg}"
    );
}

#[test]
fn remove_deletes_checkout_and_undeclares_submodule() {
    let (_tmp, sup, _v1) = fixture(&["/src/"], None);
    let dep = sup.join("ext/dep");
    assert!(picky(&sup, &["init", "ext/dep"]).status.success());
    assert!(dep.join("src").is_dir());
    let gitdir = sup.join(".git/modules/ext/dep");
    assert!(gitdir.is_dir());

    let out = picky(&sup, &["remove", "ext/dep", "--yes"]);
    assert!(
        out.status.success(),
        "{}",
        String::from_utf8_lossy(&out.stderr)
    );

    assert!(!dep.exists(), "working tree should be deleted");
    assert!(!gitdir.exists(), "submodule git dir should be deleted");
    assert!(
        git(&sup, &["ls-files", "ext/dep"]).is_empty(),
        "gitlink should be dropped from the index"
    );
    let cfg = git(&sup, &["config", "-f", ".gitmodules", "--list"]);
    assert!(
        !cfg.contains("ext/dep"),
        ".gitmodules should have no trace of ext/dep left, got: {cfg}"
    );

    // Staged, ready to commit; nothing left dangling.
    let staged = git(&sup, &["diff", "--cached", "--name-only"]);
    assert!(staged.lines().any(|l| l == ".gitmodules"));
    assert!(staged.lines().any(|l| l == "ext/dep"));
}

#[test]
fn remove_requires_explicit_paths() {
    let (_tmp, sup, _v1) = fixture(&["/src/"], None);
    let out = picky(&sup, &["remove"]);
    assert!(!out.status.success(), "remove with no paths should fail");
    // Nothing was touched; the declaration is still there.
    let path_cfg = git(
        &sup,
        &[
            "config",
            "-f",
            ".gitmodules",
            "--get",
            "submodule.ext/dep.path",
        ],
    );
    assert_eq!(path_cfg, "ext/dep", "declaration should be untouched");
}

#[test]
fn remove_without_yes_is_refused_noninteractively() {
    let (_tmp, sup, _v1) = fixture(&["/src/"], None);
    assert!(picky(&sup, &["init", "ext/dep"]).status.success());
    let dep = sup.join("ext/dep");

    // No --yes, and the test harness's stdin is not a terminal to prompt on.
    let out = picky(&sup, &["remove", "ext/dep"]);
    assert!(
        !out.status.success(),
        "remove without --yes should refuse non-interactively"
    );
    assert!(dep.exists(), "nothing should have been removed");
    let path_cfg = git(
        &sup,
        &[
            "config",
            "-f",
            ".gitmodules",
            "--get",
            "submodule.ext/dep.path",
        ],
    );
    assert_eq!(path_cfg, "ext/dep", "declaration should be untouched");
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
    // A default bump fetches only the target ref: the clone stays shallow, so
    // no full history was downloaded.
    assert_eq!(
        git(&dep, &["rev-parse", "--is-shallow-repository"]),
        "true",
        "default bump must not unshallow the submodule"
    );
}

#[test]
fn update_unshallow_flag_fetches_history() {
    let (_tmp, sup, _v1) = fixture(&["/src/"], None);
    let dep = sup.join("ext/dep");
    assert!(picky(&sup, &["init", "ext/dep"]).status.success());
    assert_eq!(git(&dep, &["rev-parse", "--is-shallow-repository"]), "true");

    let out = picky(&sup, &["update", "ext/dep", "v2", "--unshallow"]);
    assert!(
        out.status.success(),
        "{}",
        String::from_utf8_lossy(&out.stderr)
    );
    // --unshallow opts into the full history, so the clone is no longer shallow
    // and `git describe` can resolve a tag.
    assert_eq!(
        git(&dep, &["rev-parse", "--is-shallow-repository"]),
        "false",
        "--unshallow must fetch full history"
    );
    assert_eq!(git(&dep, &["describe", "--tags"]), "v2");
}

#[test]
fn update_bad_ref_fetch_suggests_unshallow() {
    // A real server-side SHA-in-want rejection can't be reproduced through
    // `file://` remotes (git's local-clone optimization skips that check
    // entirely). What's testable is the error translation: any failure of
    // the default bump's targeted fetch must carry the `--unshallow` hint,
    // which this nonexistent SHA exercises via the same code path.
    let (_tmp, sup, _v1) = fixture(&["/src/"], None);
    assert!(picky(&sup, &["init", "ext/dep"]).status.success());

    let bogus = "deadbeefdeadbeefdeadbeefdeadbeefdeadbeef";
    let out = picky(&sup, &["update", "ext/dep", bogus]);
    assert!(
        !out.status.success(),
        "fetching a nonexistent SHA should fail"
    );
    let stderr = String::from_utf8_lossy(&out.stderr);
    assert!(
        stderr.contains("--unshallow"),
        "expected a hint to retry with --unshallow, got: {stderr}"
    );
}

#[test]
fn update_all_refreshes_every_submodule_without_bumping() {
    let (tmp, url, v1) = remote_fixture();
    let sup = empty_super(&tmp);

    for name in ["ext/a", "ext/b"] {
        git(
            &sup,
            &[
                "update-index",
                "--add",
                "--cacheinfo",
                &format!("160000,{v1},{name}"),
            ],
        );
        git(
            &sup,
            &[
                "config",
                "-f",
                ".gitmodules",
                &format!("submodule.{name}.path"),
                name,
            ],
        );
        git(
            &sup,
            &[
                "config",
                "-f",
                ".gitmodules",
                &format!("submodule.{name}.url"),
                &url,
            ],
        );
    }
    git(&sup, &["add", ".gitmodules"]);
    git(&sup, &["commit", "-qm", "subs"]);

    assert!(picky(&sup, &["init"]).status.success());
    let a = sup.join("ext/a");
    let b = sup.join("ext/b");
    assert_eq!(git(&a, &["rev-parse", "HEAD"]), v1);
    assert_eq!(git(&b, &["rev-parse", "HEAD"]), v1);

    let out = picky(&sup, &["update", "--all"]);
    assert!(
        out.status.success(),
        "{}",
        String::from_utf8_lossy(&out.stderr)
    );

    // Still at v1 (a refresh, not a bump) but both freshly re-checked-out.
    assert_eq!(git(&a, &["rev-parse", "HEAD"]), v1);
    assert_eq!(git(&b, &["rev-parse", "HEAD"]), v1);

    // --all can't be combined with a target/ref.
    let out = picky(&sup, &["update", "--all", "ext/a"]);
    assert!(
        !out.status.success(),
        "--all with a target should be rejected"
    );
}

#[test]
fn status_upstream_column_reflects_refresh_cache() {
    let (tmp, url, v1) = remote_fixture();
    let sup = empty_super(&tmp);

    // Declare with a tracked branch (not just a bare pin): staleness is only
    // well-defined against a branch, not an immutable SHA/tag pin. Pin at v1
    // while the remote's "main" tip (its last commit) is v2.
    git(
        &sup,
        &[
            "update-index",
            "--add",
            "--cacheinfo",
            &format!("160000,{v1},ext/dep"),
        ],
    );
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
            "submodule.ext/dep.branch",
            "main",
        ],
    );
    git(&sup, &["add", ".gitmodules"]);
    git(&sup, &["commit", "-qm", "sub"]);
    assert!(picky(&sup, &["init", "ext/dep"]).status.success());

    // No cache yet: unknown.
    let out = picky(&sup, &["status"]);
    assert!(
        out.status.success(),
        "{}",
        String::from_utf8_lossy(&out.stderr)
    );
    let stdout = String::from_utf8_lossy(&out.stdout);
    let dep_line = |s: &str| {
        s.lines()
            .find(|l| l.contains("ext/dep"))
            .map(str::to_string)
    };
    assert!(
        dep_line(&stdout).is_some_and(|l| l.contains('?')),
        "expected an unknown upstream marker, got: {stdout}"
    );

    // Populate the cache: pinned at v1 while main's tip is v2 ⇒ stale.
    assert!(picky(&sup, &["refresh", "ext/dep"]).status.success());
    let out = picky(&sup, &["status"]);
    assert!(
        out.status.success(),
        "{}",
        String::from_utf8_lossy(&out.stderr)
    );
    let stdout = String::from_utf8_lossy(&out.stdout);
    assert!(
        dep_line(&stdout).is_some_and(|l| l.contains("stale")),
        "expected a stale upstream marker, got: {stdout}"
    );

    // Bump onto the branch tip: now current.
    assert!(picky(&sup, &["update", "ext/dep", "main"]).status.success());
    assert!(picky(&sup, &["refresh", "ext/dep"]).status.success());
    let out = picky(&sup, &["status"]);
    assert!(
        out.status.success(),
        "{}",
        String::from_utf8_lossy(&out.stderr)
    );
    let stdout = String::from_utf8_lossy(&out.stdout);
    assert!(
        dep_line(&stdout).is_some_and(|l| l.contains("current")),
        "expected a current upstream marker, got: {stdout}"
    );
}

#[test]
fn status_json_emits_a_parseable_array() {
    let (_tmp, sup, _v1) = fixture(&["/src/"], None);
    assert!(picky(&sup, &["init", "ext/dep"]).status.success());

    let out = picky(&sup, &["status", "--json"]);
    assert!(
        out.status.success(),
        "{}",
        String::from_utf8_lossy(&out.stderr)
    );
    let stdout = String::from_utf8_lossy(&out.stdout);
    let trimmed = stdout.trim();
    assert!(
        trimmed.starts_with('[') && trimmed.ends_with(']'),
        "{stdout}"
    );
    assert!(trimmed.contains(r#""submodule": "ext/dep""#), "{stdout}");
    assert!(trimmed.contains(r#""sparse": "on(1)""#), "{stdout}");
    // No table headers or the human-only footer hint leaking into JSON mode.
    assert!(!stdout.contains("SUBMODULE"), "{stdout}");
    assert!(!stdout.contains("no cached ref data"), "{stdout}");

    // No submodules ⇒ a valid empty array, not the plain-text warning.
    let (tmp2, ..) = remote_fixture();
    let empty_sup = empty_super(&tmp2);
    let out = picky(&empty_sup, &["status", "--json"]);
    assert!(out.status.success());
    assert_eq!(String::from_utf8_lossy(&out.stdout).trim(), "[]");
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
    // Git's own explanation must reach the user: a patch can fail because its
    // context drifted *or* because its target is gone, and only this text
    // tells them apart. It used to be discarded, leaving only advice to
    // resolve conflict markers that may not exist.
    let stderr = String::from_utf8_lossy(&out.stderr);
    assert!(
        stderr.contains("error: patch failed") || stderr.contains("does not exist in index"),
        "git's reason should be surfaced, got:\n{stderr}"
    );
}

/// The dirty-tree warning must fire on a real unexported edit and stay quiet
/// otherwise. Applying the patch stack is itself what makes the tree dirty, so
/// a plain `status --porcelain` check warned on every run after the first.
#[test]
fn update_warns_only_about_changes_beyond_the_patch_stack() {
    let (_tmp, sup, _v1) = fixture(&["/src/"], Some("patches"));
    let dep = sup.join("ext/dep");
    assert!(picky(&sup, &["init", "ext/dep"]).status.success());
    write(
        &sup.join("patches/0001-good.patch"),
        "--- a/src/c.txt\n+++ b/src/c.txt\n@@ -1,3 +1,3 @@\n line1\n-line2\n+line2-patched\n line3\n",
    );

    let warned = |out: &Output| {
        String::from_utf8_lossy(&out.stderr).contains("changes beyond its patch stack")
    };

    // First run applies the patch and leaves the tree dirty by design.
    let out = picky(&sup, &["update"]);
    assert!(out.status.success());
    // Second run sees exactly that same dirt: not a user edit, so no warning.
    let out = picky(&sup, &["update"]);
    assert!(out.status.success());
    assert!(!warned(&out), "patch-stack dirt must not warn");

    // An edit inside a file the patch stack already touches must still be
    // caught: the status line is unchanged, only the content differs. It is
    // refused outright, not merely warned about, so the edit survives.
    let c = dep.join("src/c.txt");
    let body = std::fs::read_to_string(&c).unwrap();
    write(&c, &format!("{body}hand-edit\n"));
    let out = picky(&sup, &["update"]);
    assert!(
        !out.status.success(),
        "an unexported edit must stop the update"
    );
    assert!(warned(&out), "the refusal should name the cause");
    assert!(std::fs::read_to_string(&c).unwrap().contains("hand-edit"));

    // `--force` discards it, with a warning.
    let out = picky(&sup, &["update", "--force"]);
    assert!(out.status.success());
    assert!(warned(&out), "a forced discard must still warn");
    assert!(!std::fs::read_to_string(&c).unwrap().contains("hand-edit"));

    // Settled again.
    let out = picky(&sup, &["update"]);
    assert!(out.status.success());
    assert!(!warned(&out), "should be quiet once reset");
}

/// A path that is a tracked directory rather than a submodule gitlink must be
/// named as such. `ls-files -s <dir>` lists every file under it, so taking the
/// first row handed back a *blob* SHA that failed far away inside git as
/// "upload-pack: not our ref".
#[test]
fn init_rejects_a_path_that_is_not_a_gitlink() {
    let (tmp, url, _v1) = remote_fixture();
    let sup = empty_super(&tmp);
    write(&sup.join("vendor/thing/f.txt"), "hi\n");
    git(&sup, &["add", "-A"]);
    git(&sup, &["commit", "-qm", "plain dir"]);
    git(
        &sup,
        &[
            "config",
            "-f",
            ".gitmodules",
            "submodule.vendor/thing.path",
            "vendor/thing",
        ],
    );
    git(
        &sup,
        &[
            "config",
            "-f",
            ".gitmodules",
            "submodule.vendor/thing.url",
            &url,
        ],
    );

    let out = picky(&sup, &["init", "vendor/thing"]);
    assert!(!out.status.success(), "a non-gitlink path should fail");
    let stderr = String::from_utf8_lossy(&out.stderr);
    assert!(
        stderr.contains("not a submodule gitlink"),
        "stderr: {stderr}"
    );
}

#[test]
fn sparse_subcommand_widens_and_narrows() {
    let (_tmp, sup, _v1) = fixture(&["/src/"], None);
    let dep = sup.join("ext/dep");
    assert!(picky(&sup, &["init", "ext/dep"]).status.success());
    assert!(dep.join("src").is_dir());
    assert!(!dep.join("keep").exists());

    // Widen: add /keep/ and reconcile.
    let out = picky(&sup, &["sparse", "add", "/keep/", "-p", "ext/dep"]);
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
    let out = picky(&sup, &["sparse", "remove", "/src/", "-p", "ext/dep"]);
    assert!(
        out.status.success(),
        "{}",
        String::from_utf8_lossy(&out.stderr)
    );
    assert!(!dep.join("src").exists(), "removed path should be trimmed");
    assert!(dep.join("keep").is_dir());
}

/// `sparse clear` must restore the *whole* checkout, not just flip the config
/// key: dropping every pattern needs `git sparse-checkout disable` to clear the
/// index's `skip-worktree` bits. Setting `core.sparseCheckout=false` alone left
/// the excluded files missing while reporting a full checkout.
#[test]
fn sparse_clear_restores_the_full_checkout() {
    let (_tmp, sup, _v1) = fixture(&["/src/"], None);
    let dep = sup.join("ext/dep");
    assert!(picky(&sup, &["init", "ext/dep"]).status.success());
    assert!(!dep.join("drop").exists(), "starts sparse");

    let out = picky(&sup, &["sparse", "clear", "-p", "ext/dep"]);
    assert!(
        out.status.success(),
        "{}",
        String::from_utf8_lossy(&out.stderr)
    );
    for path in ["src", "keep", "drop"] {
        assert!(
            dep.join(path).is_dir(),
            "{path} should materialize after clear"
        );
    }
    assert!(dep.join("README").is_file());
    // No `skip-worktree` bit may survive: `ls-files -v` marks them with `S`.
    let listed = git(&dep, &["ls-files", "-v"]);
    assert!(
        !listed.lines().any(|l| l.starts_with('S')),
        "skip-worktree bits left behind:\n{listed}"
    );
}

/// Sparse state must survive stock `git sparse-checkout` being run inside the
/// submodule. That porcelain sets `extensions.worktreeConfig` and moves
/// `core.sparseCheckout` into `config.worktree`, which outranks the plain
/// config; picky writing the key by hand meant every later `reapply` died with
/// "must be in a sparse-checkout".
#[test]
fn sparse_state_survives_stock_git_sparse_checkout() {
    let (_tmp, sup, _v1) = fixture(&["/src/"], None);
    let dep = sup.join("ext/dep");
    assert!(picky(&sup, &["init", "ext/dep"]).status.success());

    // Disable sparsity behind picky's back, as a user poking at the submodule
    // with stock git would.
    git(&dep, &["sparse-checkout", "disable"]);
    assert!(dep.join("drop").is_dir(), "stock disable materializes all");

    let out = picky(&sup, &["update", "ext/dep"]);
    assert!(
        out.status.success(),
        "update after a stock sparse-checkout disable: {}",
        String::from_utf8_lossy(&out.stderr)
    );
    assert!(
        !dep.join("drop").exists(),
        "declared patterns should be reasserted"
    );
    assert!(dep.join("src").is_dir());
}

#[test]
fn sparse_list_with_no_path_lists_every_submodule() {
    let (tmp, url, v1) = remote_fixture();
    let sup = empty_super(&tmp);

    for (name, pat) in [("ext/a", "/src/"), ("ext/b", "/keep/")] {
        git(
            &sup,
            &[
                "update-index",
                "--add",
                "--cacheinfo",
                &format!("160000,{v1},{name}"),
            ],
        );
        git(
            &sup,
            &[
                "config",
                "-f",
                ".gitmodules",
                &format!("submodule.{name}.path"),
                name,
            ],
        );
        git(
            &sup,
            &[
                "config",
                "-f",
                ".gitmodules",
                &format!("submodule.{name}.url"),
                &url,
            ],
        );
        git(
            &sup,
            &[
                "config",
                "-f",
                ".gitmodules",
                "--add",
                &format!("picky.{name}.sparse"),
                pat,
            ],
        );
    }
    git(&sup, &["add", ".gitmodules"]);
    git(&sup, &["commit", "-qm", "subs"]);

    // With no -p, list must cover both, not error out like the mutating
    // actions do when there's more than one submodule and no path given.
    let out = picky(&sup, &["sparse", "list"]);
    assert!(
        out.status.success(),
        "{}",
        String::from_utf8_lossy(&out.stderr)
    );
    let stdout = String::from_utf8_lossy(&out.stdout);
    assert!(
        stdout.contains("ext/a") && stdout.contains("/src/"),
        "{stdout}"
    );
    assert!(
        stdout.contains("ext/b") && stdout.contains("/keep/"),
        "{stdout}"
    );

    // Mutating actions still require an explicit path when ambiguous.
    let out = picky(&sup, &["sparse", "add", "/extra/"]);
    assert!(
        !out.status.success(),
        "sparse add with no path should still require -p when there's more than one submodule"
    );
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

#[test]
fn sparse_set_replaces_list_from_file_and_stdin() {
    let (tmp, sup, _v1) = fixture(&["/src/"], None);
    let dep = sup.join("ext/dep");
    assert!(picky(&sup, &["init", "ext/dep"]).status.success());

    let read = || {
        git(
            &sup,
            &[
                "config",
                "-f",
                ".gitmodules",
                "--get-all",
                "picky.ext/dep.sparse",
            ],
        )
        .lines()
        .map(str::to_string)
        .collect::<Vec<_>>()
    };

    // set from a file: replaces /src/ wholesale, ignoring blanks + comments.
    let list = tmp.path().join("patterns.txt");
    write(&list, "# keep only this\n/keep/\n\n");
    let out = picky(
        &sup,
        &[
            "sparse",
            "set",
            "--from",
            list.to_str().unwrap(),
            "-p",
            "ext/dep",
        ],
    );
    assert!(
        out.status.success(),
        "{}",
        String::from_utf8_lossy(&out.stderr)
    );
    assert_eq!(read(), vec!["/keep/".to_string()]);
    assert!(dep.join("keep").is_dir(), "set should materialize /keep/");
    assert!(!dep.join("src").exists(), "set should trim /src/");

    // set from stdin: replaces again, deduping repeated lines.
    let out = picky_stdin(
        &sup,
        &["sparse", "set", "--stdin", "-p", "ext/dep"],
        "/src/\n/src/\n",
    );
    assert!(
        out.status.success(),
        "{}",
        String::from_utf8_lossy(&out.stderr)
    );
    assert_eq!(read(), vec!["/src/".to_string()]);
    assert!(dep.join("src").is_dir());
    assert!(!dep.join("keep").exists());

    // no source ⇒ error (use `clear` to empty).
    let out = picky(&sup, &["sparse", "set", "-p", "ext/dep"]);
    assert!(!out.status.success(), "set with no patterns must fail");
}

#[test]
fn post_update_hook_runs_after_checkout() {
    let (_tmp, sup, v1) = fixture(&["/src/"], None);
    // A hook that records the env vars picky exposes to it.
    let cmd =
        "printf '%s %s\\n' \"$PICKY_SUBMODULE_PATH\" \"$PICKY_SUBMODULE_SHA\" > .picky-hook-ran";
    git(
        &sup,
        &[
            "config",
            "-f",
            ".gitmodules",
            "picky.ext/dep.postUpdate",
            cmd,
        ],
    );
    // Pre-approve it in *local* (untracked) config: simulates a user who has
    // already reviewed and trusted this exact hook text at this exact SHA.
    git(&sup, &["config", "picky.ext/dep.trustedPostUpdate", cmd]);
    git(&sup, &["config", "picky.ext/dep.trustedPostUpdateSha", &v1]);

    let out = picky(&sup, &["init", "ext/dep"]);
    assert!(
        out.status.success(),
        "{}",
        String::from_utf8_lossy(&out.stderr)
    );

    // The hook ran in the submodule worktree with the documented env vars.
    let marker = sup.join("ext/dep/.picky-hook-ran");
    assert!(marker.is_file(), "post-update hook should have run");
    let head = git(&sup.join("ext/dep"), &["rev-parse", "HEAD"]);
    let contents = std::fs::read_to_string(&marker).unwrap();
    assert_eq!(contents.trim_end(), format!("ext/dep {head}"));
}

#[test]
fn post_update_hook_failure_is_fatal() {
    let (_tmp, sup, v1) = fixture(&["/src/"], None);
    git(
        &sup,
        &[
            "config",
            "-f",
            ".gitmodules",
            "picky.ext/dep.postUpdate",
            "exit 3",
        ],
    );
    git(
        &sup,
        &["config", "picky.ext/dep.trustedPostUpdate", "exit 3"],
    );
    git(&sup, &["config", "picky.ext/dep.trustedPostUpdateSha", &v1]);

    let out = picky(&sup, &["init", "ext/dep"]);
    assert!(
        !out.status.success(),
        "a failing post-update hook must fail the command"
    );
}

#[test]
fn post_update_hook_untrusted_is_refused_noninteractively() {
    let (_tmp, sup, _v1) = fixture(&["/src/"], None);
    // Declared in .gitmodules but never approved anywhere: a hostile clone's
    // hook must not run just because the checkout succeeded.
    git(
        &sup,
        &[
            "config",
            "-f",
            ".gitmodules",
            "picky.ext/dep.postUpdate",
            "touch .should-not-run",
        ],
    );

    let out = picky(&sup, &["init", "ext/dep"]);
    assert!(
        !out.status.success(),
        "an unapproved post-update hook must not run"
    );
    assert!(
        !sup.join("ext/dep/.should-not-run").exists(),
        "hook must not have executed"
    );
    let stderr = String::from_utf8_lossy(&out.stderr);
    assert!(
        stderr.contains("trust") || stderr.contains("approve"),
        "expected trust guidance in stderr, got: {stderr}"
    );
}

#[test]
fn post_update_hook_trust_env_var_allows_and_persists() {
    let (_tmp, sup, _v1) = fixture(&["/src/"], None);
    git(
        &sup,
        &[
            "config",
            "-f",
            ".gitmodules",
            "picky.ext/dep.postUpdate",
            "touch .trusted-via-env",
        ],
    );

    let out = picky_with_env(&sup, &["init", "ext/dep"], &[("PICKY_TRUST_HOOKS", "1")]);
    assert!(
        out.status.success(),
        "{}",
        String::from_utf8_lossy(&out.stderr)
    );
    assert!(sup.join("ext/dep/.trusted-via-env").exists());

    // The env-var approval was persisted to local config, so a later run
    // doesn't need PICKY_TRUST_HOOKS again.
    std::fs::remove_file(sup.join("ext/dep/.trusted-via-env")).unwrap();
    let out = picky(&sup, &["init", "ext/dep"]);
    assert!(
        out.status.success(),
        "{}",
        String::from_utf8_lossy(&out.stderr)
    );
    assert!(
        sup.join("ext/dep/.trusted-via-env").exists(),
        "trust should persist across runs"
    );
}

#[test]
fn post_update_hook_sha_bump_invalidates_trust() {
    // A hook that runs a script living *inside* the submodule: the
    // `postUpdate` string never changes, but the script's contents can, on a
    // SHA bump. Trust pinned to command text alone would miss that; pinning
    // (text, SHA) must not.
    let (_tmp, sup, v1) = fixture(&["/src/"], None);
    let cmd = "touch .hook-ran";
    git(
        &sup,
        &[
            "config",
            "-f",
            ".gitmodules",
            "picky.ext/dep.postUpdate",
            cmd,
        ],
    );
    git(&sup, &["config", "picky.ext/dep.trustedPostUpdate", cmd]);
    git(&sup, &["config", "picky.ext/dep.trustedPostUpdateSha", &v1]);

    assert!(picky(&sup, &["init", "ext/dep"]).status.success());
    assert!(sup.join("ext/dep/.hook-ran").is_file());
    std::fs::remove_file(sup.join("ext/dep/.hook-ran")).unwrap();

    // Bump to v2: same `postUpdate` text, different SHA. The old approval
    // must not carry over.
    let out = picky(&sup, &["update", "ext/dep", "v2"]);
    assert!(
        !out.status.success(),
        "a SHA bump must re-arm the trust prompt even with identical hook text"
    );
    assert!(
        !sup.join("ext/dep/.hook-ran").exists(),
        "hook must not run under the stale SHA's approval"
    );
    let stderr = String::from_utf8_lossy(&out.stderr);
    assert!(
        stderr.contains("trust") || stderr.contains("approve"),
        "expected trust guidance in stderr, got: {stderr}"
    );

    // Re-approving at the new SHA lets it run again.
    let dep_head = git(&sup.join("ext/dep"), &["rev-parse", "HEAD"]);
    git(
        &sup,
        &["config", "picky.ext/dep.trustedPostUpdateSha", &dep_head],
    );
    let out = picky(&sup, &["update", "ext/dep", "v2"]);
    assert!(
        out.status.success(),
        "{}",
        String::from_utf8_lossy(&out.stderr)
    );
    assert!(sup.join("ext/dep/.hook-ran").is_file());
}

#[test]
fn doctor_reports_no_issues_on_a_clean_repo() {
    let (_tmp, sup, _v1) = fixture(&["/src/"], None);
    assert!(picky(&sup, &["init", "ext/dep"]).status.success());

    let out = picky(&sup, &["doctor"]);
    assert!(
        out.status.success(),
        "{}",
        String::from_utf8_lossy(&out.stderr)
    );
}

#[test]
fn doctor_flags_orphaned_gitdir_and_dangling_gitlink() {
    let (_tmp, sup, _v1) = fixture(&["/src/"], None);
    assert!(picky(&sup, &["init", "ext/dep"]).status.success());

    // Simulate hand-editing .gitmodules to drop the declaration instead of
    // running `picky remove`: the git dir under .git/modules is left behind.
    git(
        &sup,
        &[
            "config",
            "-f",
            ".gitmodules",
            "--remove-section",
            "submodule.ext/dep",
        ],
    );
    let out = picky(&sup, &["doctor"]);
    assert!(
        out.status.success(),
        "doctor is diagnostic-only, never fails"
    );
    let stderr = String::from_utf8_lossy(&out.stderr);
    assert!(
        stderr.contains("orphaned git dir") && stderr.contains(".git/modules/ext/dep"),
        "expected an orphaned-gitdir warning, got: {stderr}"
    );

    // Simulate a dangling gitlink on a *different*, still-declared submodule:
    // the worktree `.git` file survives but its target git dir is gone.
    let (_tmp2, sup2, _v1_2) = fixture(&["/src/"], None);
    assert!(picky(&sup2, &["init", "ext/dep"]).status.success());
    std::fs::remove_dir_all(sup2.join(".git/modules/ext/dep")).unwrap();
    let out2 = picky(&sup2, &["doctor"]);
    assert!(out2.status.success());
    let stderr2 = String::from_utf8_lossy(&out2.stderr);
    assert!(
        stderr2.contains("dangling gitlink"),
        "expected a dangling-gitlink warning, got: {stderr2}"
    );
}

#[test]
fn doctor_strict_exits_nonzero_on_issues() {
    let (_tmp, sup, _v1) = fixture(&["/src/"], None);
    assert!(picky(&sup, &["init", "ext/dep"]).status.success());

    // Clean repo: --strict still exits 0.
    assert!(picky(&sup, &["doctor", "--strict"]).status.success());

    // Same orphan scenario as above, but with --strict this time.
    git(
        &sup,
        &[
            "config",
            "-f",
            ".gitmodules",
            "--remove-section",
            "submodule.ext/dep",
        ],
    );
    let out = picky(&sup, &["doctor", "--strict"]);
    assert!(
        !out.status.success(),
        "--strict should exit nonzero when issues are found"
    );
    let out_plain = picky(&sup, &["doctor"]);
    assert!(
        out_plain.status.success(),
        "without --strict the same issues should still just warn"
    );
}

#[test]
fn an_unparseable_gitmodules_is_an_error_not_an_empty_config() {
    // Read as "nothing declared", a syntax error would surface as `no
    // submodules declared` and `doctor --strict` would pass on a broken file.
    let (tmp, ..) = remote_fixture();
    let sup = empty_super(&tmp);
    // Unterminated section header.
    write(
        &sup.join(".gitmodules"),
        "[submodule \"ext/dep\"\n\tpath = ext/dep\n",
    );

    for args in [
        vec!["status"],
        vec!["init"],
        vec!["doctor"],
        vec!["doctor", "--strict"],
    ] {
        let out = picky(&sup, &args);
        assert!(
            !out.status.success(),
            "`picky {args:?}` should fail on an unparseable .gitmodules"
        );
        let stderr = String::from_utf8_lossy(&out.stderr);
        assert!(
            stderr.contains(".gitmodules"),
            "`picky {args:?}` should name the file, got: {stderr}"
        );
    }

    // An *absent* file is still an empty config, not an error.
    std::fs::remove_file(sup.join(".gitmodules")).unwrap();
    assert!(picky(&sup, &["status"]).status.success());
    // So is a present-but-entryless one.
    write(&sup.join(".gitmodules"), "");
    assert!(picky(&sup, &["status"]).status.success());
}

#[test]
fn refresh_moves_a_checkout_onto_a_pin_someone_else_bumped() {
    // A refresh must target the gitlink pin, not the submodule's current HEAD:
    // pulling a superproject commit that moved a pin is what `update --all`
    // exists for.
    let (_tmp, sup, v1) = fixture(&["/src/"], None);
    let dep = sup.join("ext/dep");
    assert!(picky(&sup, &["init", "ext/dep"]).status.success());
    assert_eq!(git(&dep, &["rev-parse", "HEAD"]), v1);

    // Move the pin in the index only, as pulling someone's bump does. v2 comes
    // from the remote: the submodule was fetched shallow at v1.
    let v2 = git(
        &sup.parent().unwrap().join("remote.git"),
        &["rev-parse", "v2"],
    );
    git(
        &sup,
        &[
            "update-index",
            "--cacheinfo",
            &format!("160000,{v2},ext/dep"),
        ],
    );

    let out = picky_loud(&sup, &["update", "--all"]);
    assert!(
        out.status.success(),
        "{}",
        String::from_utf8_lossy(&out.stderr)
    );
    assert_eq!(
        git(&dep, &["rev-parse", "HEAD"]),
        v2,
        "a refresh must move the checkout onto the pin"
    );
    assert!(
        std::fs::read_to_string(dep.join("src/c.txt"))
            .unwrap()
            .contains("line4"),
        "the working tree must hold v2's content"
    );

    let stdout = String::from_utf8_lossy(&out.stdout);
    assert!(stdout.contains("moved onto its pin"), "{stdout}");

    // Re-running is a clean no-op, and says so.
    let out = picky_loud(&sup, &["update", "ext/dep"]);
    assert!(out.status.success());
    assert_eq!(git(&dep, &["rev-parse", "HEAD"]), v2);
    let stdout = String::from_utf8_lossy(&out.stdout);
    assert!(stdout.contains("already on its pin"), "{stdout}");
}

#[test]
fn refresh_fetches_a_pinned_commit_it_has_never_seen() {
    // The pin may name a commit this shallow checkout never fetched, so a
    // refresh must fetch before checking out.
    let (_tmp, sup, v1) = fixture(&["/src/"], None);
    let dep = sup.join("ext/dep");
    assert!(picky(&sup, &["init", "ext/dep"]).status.success());

    // A shallow fetch of v1 brought no tags and no later commits, so v2 is
    // absent from the submodule's object store.
    let v2 = git(
        &sup.parent().unwrap().join("remote.git"),
        &["rev-parse", "v2"],
    );
    assert!(
        !std::process::Command::new("git")
            .arg("-C")
            .arg(&dep)
            .args(["cat-file", "-e", &format!("{v2}^{{commit}}")])
            .env("GIT_NO_LAZY_FETCH", "1")
            .status()
            .unwrap()
            .success(),
        "fixture precondition: v2 should not be present locally yet"
    );
    git(
        &sup,
        &[
            "update-index",
            "--cacheinfo",
            &format!("160000,{v2},ext/dep"),
        ],
    );
    assert_ne!(git(&dep, &["rev-parse", "HEAD"]), v2);
    assert_eq!(git(&dep, &["rev-parse", "HEAD"]), v1);

    let out = picky(&sup, &["update", "ext/dep"]);
    assert!(
        out.status.success(),
        "{}",
        String::from_utf8_lossy(&out.stderr)
    );
    assert_eq!(git(&dep, &["rev-parse", "HEAD"]), v2);
}

#[test]
fn a_relative_gitmodules_url_resolves_like_git_does() {
    // Git resolves `url = ../x` against the superproject's remote, not the
    // submodule directory.
    let (tmp, url, v1) = remote_fixture();
    let sup = empty_super(&tmp);

    // The remote sits next to the superproject, so `../remote.git` names it
    // from there — and something else from inside `ext/dep`.
    let relative = "../remote.git";
    git(
        &sup,
        &[
            "update-index",
            "--add",
            "--cacheinfo",
            &format!("160000,{v1},ext/dep"),
        ],
    );
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
        &[
            "config",
            "-f",
            ".gitmodules",
            "submodule.ext/dep.url",
            relative,
        ],
    );
    // Git needs a remote on the superproject to resolve against.
    git(&sup, &["remote", "add", "origin", &url]);
    git(&sup, &["add", ".gitmodules"]);
    git(&sup, &["commit", "-qm", "sub"]);

    let out = picky(&sup, &["init", "ext/dep"]);
    assert!(
        out.status.success(),
        "relative url should resolve: {}",
        String::from_utf8_lossy(&out.stderr)
    );
    assert_eq!(git(&sup.join("ext/dep"), &["rev-parse", "HEAD"]), v1);
    // `origin` inside the submodule is the resolved URL, not the raw `../…`.
    let origin = git(&sup.join("ext/dep"), &["remote", "get-url", "origin"]);
    assert!(!origin.starts_with("../"), "unresolved origin: {origin}");
}

#[test]
fn a_zero_depth_is_refused_before_any_work_happens() {
    let (tmp, url, _v1) = remote_fixture();
    let sup = empty_super(&tmp);

    // Clap rejects the flag outright, so nothing is created.
    let out = picky(&sup, &["add", "--depth", "0", &url, "ext/dep"]);
    assert!(!out.status.success());
    assert!(
        !sup.join("ext/dep").exists(),
        "a rejected add built a git dir"
    );
    assert!(!sup.join(".git/modules/ext/dep").exists());

    // A `picky.<name>.depth = 0` is reported too, rather than reaching
    // `git fetch --depth=0` after the git dir is already built.
    let (_tmp2, sup2, _v1) = fixture(&["/src/"], None);
    git(
        &sup2,
        &["config", "-f", ".gitmodules", "picky.ext/dep.depth", "0"],
    );
    let out = picky(&sup2, &["status"]);
    assert!(!out.status.success());
    let stderr = String::from_utf8_lossy(&out.stderr);
    assert!(stderr.contains("depth"), "{stderr}");
}

#[test]
fn doctor_does_not_follow_symlinks_under_git_modules() {
    // `is_dir()` follows symlinks, which sends the scan recursing through
    // itself and reporting phantom `a/loop/a/loop/…` orphaned git dirs.
    let (tmp, ..) = remote_fixture();
    let sup = empty_super(&tmp);
    let modules = sup.join(".git/modules/a");
    std::fs::create_dir_all(&modules).unwrap();
    std::os::unix::fs::symlink("../../modules", modules.join("loop")).unwrap();

    let out = picky(&sup, &["doctor"]);
    assert!(
        out.status.success(),
        "{}",
        String::from_utf8_lossy(&out.stderr)
    );
    let stderr = String::from_utf8_lossy(&out.stderr);
    assert!(
        !stderr.contains("loop/a/loop"),
        "symlink was traversed: {stderr}"
    );
}

#[test]
fn add_refuses_to_check_out_over_unrelated_content() {
    let (tmp, url, _v1) = remote_fixture();
    let sup = empty_super(&tmp);
    let occupied = sup.join("occupied");
    write(&occupied.join("important.txt"), "mine\n");

    let out = picky(&sup, &["add", &url, "occupied"]);
    assert!(!out.status.success());
    let stderr = String::from_utf8_lossy(&out.stderr);
    assert!(stderr.contains("already exists"), "{stderr}");
    assert_eq!(
        std::fs::read_to_string(occupied.join("important.txt")).unwrap(),
        "mine\n",
        "the pre-existing content must be untouched"
    );
    // An empty directory is fine (git leaves those for uninitialized
    // submodules), as is re-adding over picky's own checkout.
    let empty = sup.join("ext/dep");
    std::fs::create_dir_all(&empty).unwrap();
    assert!(picky(&sup, &["add", &url, "ext/dep"]).status.success());
    assert!(picky(&sup, &["add", &url, "ext/dep"]).status.success());
}

#[test]
fn status_reports_the_object_store_separately_and_as_json_numbers() {
    // The worktree's `.git` is a link file, so a walk of the checkout cannot
    // see the object store; it is reported separately.
    let (_tmp, sup, _v1) = fixture(&["/src/"], None);
    assert!(picky(&sup, &["init", "ext/dep"]).status.success());

    let out = picky(&sup, &["status"]);
    let stdout = String::from_utf8_lossy(&out.stdout);
    assert!(stdout.contains("STORE"), "no STORE column: {stdout}");

    let out = picky(&sup, &["status", "--json"]);
    let json = String::from_utf8_lossy(&out.stdout);
    // Sizes are bare numbers, not `"77M"`, so a script can use them.
    let size = json
        .split(r#""size": "#)
        .nth(1)
        .and_then(|s| s.split(&[',', '}'][..]).next())
        .unwrap_or("")
        .trim()
        .to_string();
    assert!(
        size.parse::<u64>().is_ok_and(|n| n > 0),
        "size should be a positive number, got {size:?} in {json}"
    );
    let store = json
        .split(r#""store": "#)
        .nth(1)
        .and_then(|s| s.split(&[',', '}'][..]).next())
        .unwrap_or("")
        .trim()
        .to_string();
    assert!(
        store.parse::<u64>().is_ok_and(|n| n > 0),
        "store should be a positive number, got {store:?} in {json}"
    );
    assert!(!json.contains(r#""size": ""#), "size is quoted: {json}");
}

#[test]
fn status_sizes_are_exact_byte_counts() {
    // Sizes come from a directory walk, so they are exact byte counts rather
    // than 1 KiB blocks.
    let (_tmp, sup, _v1) = fixture(&["/src/"], None);
    assert!(picky(&sup, &["init", "ext/dep"]).status.success());

    let json = String::from_utf8_lossy(&picky(&sup, &["status", "--json"]).stdout).to_string();
    let field = |key: &str| -> u64 {
        json.split(&format!(r#""{key}": "#))
            .nth(1)
            .and_then(|s| s.split(&[',', '}'][..]).next())
            .unwrap()
            .trim()
            .parse()
            .unwrap()
    };
    let (size, store) = (field("size"), field("store"));
    assert!(size > 0 && store > 0, "{json}");
    // A partial clone's object store outweighs a sparse working tree.
    assert!(store > size, "store {store} should exceed worktree {size}");
}

#[test]
fn doctor_reports_sparse_drift_from_the_declaration() {
    // `init`/`update` reconcile drift silently, so `doctor` is what reports a
    // `git sparse-checkout` run by hand inside the submodule.
    let (_tmp, sup, _v1) = fixture(&["/keep/", "/src/"], None);
    let dep = sup.join("ext/dep");
    assert!(picky(&sup, &["init", "ext/dep"]).status.success());

    // Clean to start with.
    let out = picky(&sup, &["doctor", "--strict"]);
    assert!(
        out.status.success(),
        "{}",
        String::from_utf8_lossy(&out.stderr)
    );

    // Patterns changed underneath picky.
    git(
        &dep,
        &[
            "sparse-checkout",
            "set",
            "--no-cone",
            "--skip-checks",
            "--",
            "/keep/",
        ],
    );
    let out = picky(&sup, &["doctor"]);
    let stderr = String::from_utf8_lossy(&out.stderr);
    assert!(stderr.contains("drifted"), "{stderr}");
    assert!(
        stderr.contains("/src/"),
        "should name the missing pattern: {stderr}"
    );
    assert!(!picky(&sup, &["doctor", "--strict"]).status.success());

    // `init` reconciles it, and `doctor` goes quiet again.
    assert!(picky(&sup, &["init", "ext/dep"]).status.success());
    assert!(picky(&sup, &["doctor", "--strict"]).status.success());

    // Sparsity switched off entirely while patterns are still declared.
    git(&dep, &["sparse-checkout", "disable"]);
    let out = picky(&sup, &["doctor"]);
    let stderr = String::from_utf8_lossy(&out.stderr);
    assert!(stderr.contains("not sparse"), "{stderr}");

    // And the inverse: nothing declared, but the checkout is sparse.
    assert!(
        picky(&sup, &["sparse", "clear", "-p", "ext/dep"])
            .status
            .success()
    );
    git(
        &dep,
        &[
            "sparse-checkout",
            "set",
            "--no-cone",
            "--skip-checks",
            "--",
            "/keep/",
        ],
    );
    let out = picky(&sup, &["doctor"]);
    let stderr = String::from_utf8_lossy(&out.stderr);
    assert!(stderr.contains("full checkout"), "{stderr}");
}

/// A `.gitmodules` URL is attacker-controlled in a hostile clone. Passed to
/// `git ls-remote` without a `--`, a value like `--upload-pack=<cmd>` parses as
/// an option and `<cmd>` runs against the superproject's own (local) remote:
/// code execution from `picky refresh`, or from a TAB press.
#[test]
fn refresh_does_not_run_an_option_shaped_url() {
    let (tmp, _url, _v1) = remote_fixture();
    let sup = empty_super(&tmp);
    // The injected option needs a local remote to act on: the cwd's `origin`.
    let remote = tmp.path().join("remote.git");
    git(&sup, &["remote", "add", "origin", remote.to_str().unwrap()]);
    let marker = tmp.path().join("pwned");
    git(
        &sup,
        &["config", "-f", ".gitmodules", "submodule.x.path", "x"],
    );
    git(
        &sup,
        &[
            "config",
            "-f",
            ".gitmodules",
            "submodule.x.url",
            &format!("--upload-pack=touch {};git-upload-pack", marker.display()),
        ],
    );

    let _ = picky(&sup, &["refresh"]);
    assert!(!marker.exists(), "an option-shaped URL ran a command");
}

/// `picky sparse` reconciles the checkout after editing the patterns. Going
/// through `init` (which checks out pristine upstream) silently dropped a
/// patch stack `update` had applied.
#[test]
fn sparse_edit_keeps_the_patch_stack() {
    let (_tmp, sup, _v1) = fixture(&["/src/"], Some("patches"));
    let dep = sup.join("ext/dep");
    assert!(picky(&sup, &["init", "ext/dep"]).status.success());
    write(
        &sup.join("patches/0001-good.patch"),
        "--- a/src/c.txt\n+++ b/src/c.txt\n@@ -1,3 +1,3 @@\n line1\n-line2\n+line2-patched\n line3\n",
    );
    assert!(picky(&sup, &["update"]).status.success());

    let out = picky(&sup, &["sparse", "add", "/keep/"]);
    assert!(
        out.status.success(),
        "{}",
        String::from_utf8_lossy(&out.stderr)
    );
    assert!(
        dep.join("keep/a.txt").exists(),
        "widened path should appear"
    );
    assert!(
        std::fs::read_to_string(dep.join("src/c.txt"))
            .unwrap()
            .contains("line2-patched"),
        "the patch stack must survive a sparse edit"
    );
}

/// A promisor remote's `git fetch` falls back to `partialclonefilter` when no
/// `--filter` is passed, so switching a declaration to `filter = none` must
/// clear the key, not leave the old `blob:none` silently in force.
#[test]
fn filter_none_clears_a_previous_partial_clone_filter() {
    let (_tmp, sup, _v1) = fixture(&["/src/"], None);
    let dep = sup.join("ext/dep");
    assert!(picky(&sup, &["init", "ext/dep"]).status.success());
    assert_eq!(
        git(
            &dep,
            &["config", "--get", "remote.origin.partialclonefilter"]
        ),
        "blob:none"
    );

    git(
        &sup,
        &[
            "config",
            "-f",
            ".gitmodules",
            "picky.ext/dep.filter",
            "none",
        ],
    );
    assert!(picky(&sup, &["init", "ext/dep"]).status.success());
    let still_set = Command::new("git")
        .arg("-C")
        .arg(&dep)
        .args(["config", "--get", "remote.origin.partialclonefilter"])
        .output()
        .unwrap();
    assert!(
        !still_set.status.success(),
        "stale filter left behind: {}",
        String::from_utf8_lossy(&still_set.stdout)
    );
}

/// `check_path` is lexical; a committed symlink as a leading component sends
/// `root.join(path)` outside the superproject, where `remove` deletes.
#[cfg(unix)]
#[test]
fn remove_refuses_a_path_through_a_symlink() {
    let (tmp, url, _v1) = remote_fixture();
    let sup = empty_super(&tmp);
    let outside = tmp.path().join("outside");
    write(&outside.join("dep/precious.txt"), "keep me\n");
    std::os::unix::fs::symlink(&outside, sup.join("ext")).unwrap();
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

    let out = picky(&sup, &["remove", "ext/dep", "--yes"]);
    assert!(!out.status.success(), "a symlinked path should be refused");
    assert!(
        String::from_utf8_lossy(&out.stderr).contains("symlink"),
        "stderr: {}",
        String::from_utf8_lossy(&out.stderr)
    );
    assert!(
        outside.join("dep/precious.txt").exists(),
        "nothing outside may be deleted"
    );
}

/// `PICKY_TRUST_HOOKS=0` is an explicit opt-out; a bare presence check read
/// it as blanket approval.
#[test]
fn post_update_hook_trust_env_var_zero_does_not_trust() {
    let (_tmp, sup, _v1) = fixture(&["/src/"], None);
    git(
        &sup,
        &[
            "config",
            "-f",
            ".gitmodules",
            "picky.ext/dep.postUpdate",
            "touch .should-not-run",
        ],
    );
    let out = picky_with_env(&sup, &["init", "ext/dep"], &[("PICKY_TRUST_HOOKS", "0")]);
    assert!(
        !out.status.success(),
        "PICKY_TRUST_HOOKS=0 must not approve"
    );
    assert!(!sup.join("ext/dep/.should-not-run").exists());
}

/// ssh prompts on `/dev/tty` itself, past `GIT_TERMINAL_PROMPT=0`, which
/// lands a passphrase prompt in the middle of a shell completion. Unless the
/// user configured their own ssh command, `ls-remote` runs ssh in batch mode.
#[cfg(unix)]
#[test]
fn refresh_runs_ssh_in_batch_mode() {
    use std::os::unix::fs::PermissionsExt;
    let (tmp, _url, _v1) = remote_fixture();
    let sup = empty_super(&tmp);
    let bin = tmp.path().join("bin");
    let log = tmp.path().join("ssh-args");
    write(
        &bin.join("ssh"),
        &format!("#!/bin/sh\necho \"$@\" >> '{}'\nexit 1\n", log.display()),
    );
    std::fs::set_permissions(bin.join("ssh"), std::fs::Permissions::from_mode(0o755)).unwrap();
    git(
        &sup,
        &["config", "-f", ".gitmodules", "submodule.x.path", "x"],
    );
    git(
        &sup,
        &[
            "config",
            "-f",
            ".gitmodules",
            "submodule.x.url",
            "ssh://example.invalid/x.git",
        ],
    );

    let path = format!("{}:{}", bin.display(), std::env::var("PATH").unwrap());
    let out = Command::new(BIN)
        .args(["--quiet", "refresh"])
        .current_dir(&sup)
        .env("PATH", path)
        .env("XDG_CACHE_HOME", sup.join(".cache"))
        .env("GIT_CONFIG_GLOBAL", "/dev/null")
        .env_remove("GIT_SSH_COMMAND")
        .env_remove("GIT_SSH")
        .output()
        .unwrap();
    assert!(String::from_utf8_lossy(&out.stderr).contains("could not reach"));
    let args = std::fs::read_to_string(&log).expect("fake ssh should have run");
    assert!(args.contains("BatchMode=yes"), "ssh args: {args}");
}

/// A superproject declaring and committing each of `names` at v1 from the
/// shared remote. Returns (tmp, super_dir, remote_dir).
fn multi_fixture(names: &[&str]) -> (TmpDir, PathBuf, PathBuf) {
    let (tmp, url, v1) = remote_fixture();
    let sup = empty_super(&tmp);
    for name in names {
        let gitlink = format!("160000,{v1},{name}");
        git(&sup, &["update-index", "--add", "--cacheinfo", &gitlink]);
        git(
            &sup,
            &[
                "config",
                "-f",
                ".gitmodules",
                &format!("submodule.{name}.path"),
                name,
            ],
        );
        git(
            &sup,
            &[
                "config",
                "-f",
                ".gitmodules",
                &format!("submodule.{name}.url"),
                &url,
            ],
        );
    }
    git(&sup, &["add", ".gitmodules"]);
    git(&sup, &["commit", "-qm", "subs"]);
    let remote = tmp.path().join("remote.git");
    (tmp, sup, remote)
}

/// One failing submodule must not leave the rest of `update --all` undone.
#[test]
fn update_all_carries_on_past_a_failing_submodule() {
    let (_tmp, sup, remote) = multi_fixture(&["ext/a", "ext/b"]);
    assert!(picky(&sup, &["init"]).status.success());

    // `a` holds an unexported edit, so its refresh is refused…
    let a_file = sup.join("ext/a/README");
    write(&a_file, "hand-edit\n");
    // …while `b`'s pin moved and needs catching up.
    let v2 = git(&remote, &["rev-parse", "v2"]);
    git(
        &sup,
        &["update-index", "--cacheinfo", &format!("160000,{v2},ext/b")],
    );

    let out = picky(&sup, &["update", "--all"]);
    let stderr = String::from_utf8_lossy(&out.stderr);
    assert!(
        !out.status.success(),
        "a failed submodule must fail the run"
    );
    assert!(
        stderr.contains("1 of 2 submodule(s) failed: ext/a"),
        "{stderr}"
    );
    assert_eq!(std::fs::read_to_string(&a_file).unwrap(), "hand-edit\n");
    assert_eq!(
        git(&sup.join("ext/b"), &["rev-parse", "HEAD"]),
        v2,
        "b must still be refreshed"
    );
}

/// Several submodules check out concurrently; one with an unreachable remote
/// fails on its own without taking the others down.
#[test]
fn init_of_many_checks_out_the_rest_when_one_fails() {
    let names = ["ext/a", "ext/b", "ext/c", "ext/d", "ext/e"];
    let (_tmp, sup, _remote) = multi_fixture(&names);
    git(
        &sup,
        &[
            "config",
            "-f",
            ".gitmodules",
            "submodule.ext/c.url",
            "file:///nonexistent/remote.git",
        ],
    );

    let out = picky(&sup, &["init"]);
    let stderr = String::from_utf8_lossy(&out.stderr);
    assert!(!out.status.success());
    assert!(
        stderr.contains("1 of 5 submodule(s) failed: ext/c"),
        "{stderr}"
    );
    for name in names.iter().filter(|n| **n != "ext/c") {
        assert!(
            sup.join(name).join("README").exists(),
            "{name} should be checked out"
        );
    }
}

/// `picky status --json`'s `store` field for the first row.
fn status_store(sup: &Path) -> String {
    let out = picky(sup, &["status", "--json"]);
    let json = String::from_utf8_lossy(&out.stdout).to_string();
    let at = json.find("\"store\": ").expect("store field") + 9;
    json[at..]
        .split([',', '}'])
        .next()
        .unwrap()
        .trim()
        .to_string()
}

/// Git keys a submodule's git dir by *name*; `git submodule absorbgitdirs`,
/// `deinit` and friends look there. picky used the path, which only matched
/// git when the two happened to be equal.
#[test]
fn a_named_submodule_keeps_its_git_dir_under_its_name() {
    let (tmp, url, _v1) = remote_fixture();
    let sup = empty_super(&tmp);
    let out = picky(&sup, &["add", &url, "ext/dep", "--name", "dep"]);
    assert!(
        out.status.success(),
        "{}",
        String::from_utf8_lossy(&out.stderr)
    );
    let modules = sup.join(".git/modules");
    assert!(
        modules.join("dep").is_dir(),
        "git dir should be keyed by name"
    );
    assert!(!modules.join("ext/dep").exists());

    assert_ne!(status_store(&sup), "null");
    let out = picky(&sup, &["doctor", "--strict"]);
    assert!(
        out.status.success(),
        "{}",
        String::from_utf8_lossy(&out.stderr)
    );

    assert!(
        picky(&sup, &["remove", "ext/dep", "--yes"])
            .status
            .success()
    );
    assert!(
        !modules.join("dep").exists(),
        "remove should delete the git dir"
    );
}

/// A checkout built under the older `.git/modules/<path>` layout keeps
/// working: found through its `.git` link, not reported as an orphan, and
/// cleaned up by `remove`.
#[test]
fn a_git_dir_in_the_older_path_layout_is_still_found() {
    let (tmp, url, _v1) = remote_fixture();
    let sup = empty_super(&tmp);
    assert!(
        picky(&sup, &["add", &url, "ext/dep", "--name", "dep"])
            .status
            .success()
    );
    // Move it to where earlier picky versions put it, and relink.
    let modules = sup.join(".git/modules");
    let legacy = modules.join("ext/dep");
    std::fs::create_dir_all(legacy.parent().unwrap()).unwrap();
    std::fs::rename(modules.join("dep"), &legacy).unwrap();
    write(
        &sup.join("ext/dep/.git"),
        &format!("gitdir: {}\n", legacy.display()),
    );

    assert_ne!(status_store(&sup), "null");
    let out = picky(&sup, &["doctor", "--strict"]);
    assert!(
        out.status.success(),
        "{}",
        String::from_utf8_lossy(&out.stderr)
    );

    assert!(
        picky(&sup, &["remove", "ext/dep", "--yes"])
            .status
            .success()
    );
    assert!(!legacy.exists(), "remove should delete the linked git dir");
}

/// The name now picks a directory that gets deleted recursively, so one that
/// escapes `.git/modules` is refused before any work happens.
#[test]
fn add_refuses_a_name_escaping_the_modules_dir() {
    let (tmp, url, _v1) = remote_fixture();
    let sup = empty_super(&tmp);
    let out = picky(&sup, &["add", &url, "ext/dep", "--name", "../../escape"]);
    assert!(!out.status.success());
    assert!(String::from_utf8_lossy(&out.stderr).contains("must stay inside .git/modules"));
    assert!(!sup.join("ext/dep").exists(), "nothing should be built");
}
