//! End-to-end tests for `ccr resume <id>` — the refuse / --force / exec paths
//! that unit tests can't reach (run_resume execs a real process and exits).
//! Each test builds an isolated session store via the CCR_*_DIR overrides and
//! a fake `claude` binary on PATH, so nothing real is scanned or resumed.
//! The live-guard tests require a working `pgrep` and a readable process
//! table (present on GH-hosted runners; slim containers without procps fail
//! by design — the guard is equally dead there).
#![cfg(unix)]

use std::fs;
use std::path::{Path, PathBuf};
use std::process::{Command, Output};

/// Fake-claude exit code — distinctive so a propagated code is unmistakable.
const FAKE_EXIT: i32 = 7;

struct Fixture {
    root: PathBuf,
    id: String,
}

impl Fixture {
    /// Isolated store + fake `claude` under a unique temp root.
    fn new(name: &str) -> Self {
        let id = format!("ccr-e2e-{name}-{}", std::process::id());
        let root =
            std::env::temp_dir().join(format!("ccr-resume-cli-{name}-{}", std::process::id()));
        let proj = root.join("projects").join("proj");
        fs::create_dir_all(&proj).unwrap();
        fs::create_dir_all(root.join("cwd")).unwrap();
        let bin = root.join("bin");
        fs::create_dir_all(&bin).unwrap();

        fs::write(
            proj.join(format!("{id}.jsonl")),
            format!(
                "{{\"type\":\"user\",\"cwd\":\"{}\",\"timestamp\":\"2026-06-30T10:00:00Z\",\"message\":{{\"content\":\"e2e\"}}}}\n",
                root.join("cwd").display()
            ),
        )
        .unwrap();

        let fake = bin.join("claude");
        fs::write(
            &fake,
            format!("#!/bin/sh\necho \"FAKE argv: $@\"\nexit {FAKE_EXIT}\n"),
        )
        .unwrap();
        use std::os::unix::fs::PermissionsExt;
        fs::set_permissions(&fake, fs::Permissions::from_mode(0o755)).unwrap();

        Self { root, id }
    }

    fn run(&self, args: &[&str]) -> Output {
        let path = format!(
            "{}:{}",
            self.root.join("bin").display(),
            std::env::var("PATH").unwrap_or_default()
        );
        Command::new(env!("CARGO_BIN_EXE_ccr"))
            .args(args)
            .env("CCR_CLAUDE_DIR", self.root.join("projects"))
            .env("CCR_CODEX_DIR", self.root.join("no-codex"))
            .env("CCR_GEMINI_DIR", self.root.join("no-gemini"))
            .env("PATH", path)
            .output()
            .expect("run ccr")
    }
}

impl Drop for Fixture {
    fn drop(&mut self) {
        fs::remove_dir_all(&self.root).ok();
    }
}

/// A live process whose argv carries `--resume <id>`, killed on drop.
struct Decoy(std::process::Child);

impl Decoy {
    fn spawn(id: &str) -> Self {
        // "; :" keeps this a compound command — a single command would be
        // exec'd directly by sh, replacing the argv that carries `--resume`.
        let child = Command::new("sh")
            .args(["-c", "sleep 10; :", "decoy-argv0", "--resume", id])
            .spawn()
            .expect("spawn decoy");
        Self(child)
    }
}

impl Drop for Decoy {
    fn drop(&mut self) {
        self.0.kill().ok();
        self.0.wait().ok();
    }
}

fn stderr_of(out: &Output) -> String {
    String::from_utf8_lossy(&out.stderr).into_owned()
}

#[test]
fn unknown_id_errors_without_spawning() {
    let fx = Fixture::new("unknown");
    let out = fx.run(&["resume", "no-such-id"]);
    assert_eq!(out.status.code(), Some(1));
    assert!(stderr_of(&out).contains("no session with id"));
    assert!(!String::from_utf8_lossy(&out.stdout).contains("FAKE argv"));
}

#[test]
fn resume_execs_backend_and_propagates_exit_code() {
    let fx = Fixture::new("happy");
    let out = fx.run(&["resume", &fx.id]);
    assert_eq!(
        out.status.code(),
        Some(FAKE_EXIT),
        "stderr: {}",
        stderr_of(&out)
    );
    let stdout = String::from_utf8_lossy(&out.stdout);
    assert!(stdout.contains(&format!("FAKE argv: --resume {}", fx.id)));
}

#[test]
fn refuses_live_session_without_force() {
    let fx = Fixture::new("refuse");
    let _decoy = Decoy::spawn(&fx.id);
    // Poll: the decoy must become visible to pgrep before the refusal fires.
    let mut refused = None;
    for _ in 0..25 {
        let out = fx.run(&["resume", &fx.id]);
        if out.status.code() == Some(1) && stderr_of(&out).contains("may already be running") {
            refused = Some(out);
            break;
        }
        std::thread::sleep(std::time::Duration::from_millis(100));
    }
    let out = refused.expect("resume must refuse while a --resume <id> process is live");
    let err = stderr_of(&out);
    assert!(
        err.contains("--force"),
        "refusal must mention the override: {err}"
    );
    // Refusal must happen INSTEAD of spawning: no fake-claude output.
    assert!(!String::from_utf8_lossy(&out.stdout).contains("FAKE argv"));
}

#[test]
fn force_overrides_live_guard() {
    let fx = Fixture::new("force");
    let _decoy = Decoy::spawn(&fx.id);
    // First prove the guard is actually firing (decoy visible to pgrep) —
    // otherwise a pass here would only show that --force doesn't break the
    // happy path, not that it overrides anything.
    let mut guard_active = false;
    for _ in 0..25 {
        let out = fx.run(&["resume", &fx.id]);
        if out.status.code() == Some(1) && stderr_of(&out).contains("may already be running") {
            guard_active = true;
            break;
        }
        std::thread::sleep(std::time::Duration::from_millis(100));
    }
    assert!(
        guard_active,
        "guard never fired; --force override untestable"
    );
    let out = fx.run(&["resume", &fx.id, "--force"]);
    assert_eq!(
        out.status.code(),
        Some(FAKE_EXIT),
        "stderr: {}",
        stderr_of(&out)
    );
    assert!(String::from_utf8_lossy(&out.stdout).contains("FAKE argv"));
}

/// The store override must also keep `path` working against the fixture —
/// guards the CCR_CLAUDE_DIR plumbing the other tests depend on.
#[test]
fn path_resolves_inside_fixture_store() {
    let fx = Fixture::new("path");
    let out = fx.run(&["path", &fx.id]);
    assert_eq!(out.status.code(), Some(0));
    let printed = String::from_utf8_lossy(&out.stdout);
    assert!(Path::new(printed.trim()).starts_with(&fx.root));
}
