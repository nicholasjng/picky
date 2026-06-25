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
    picky_with_env(dir, args, &[])
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
    git(&sup, &["config", "picky.ext/dep.trustedPostUpdateSha", &dep_head]);
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
