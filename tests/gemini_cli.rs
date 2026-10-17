//! Exercise both Gemini session formats without launching the installed CLI.
#![cfg(unix)]

use serde_json::{Value, json};
use std::fs;
use std::os::unix::fs::PermissionsExt;
use std::path::PathBuf;
use std::process::{Command, Output};

struct Fixture {
    root: PathBuf,
}

impl Fixture {
    fn new(name: &str) -> Self {
        let root = std::env::temp_dir().join(format!("ccr-gemini-{name}-{}", std::process::id()));
        let chats = root.join("gemini/tmp/project/chats");
        fs::create_dir_all(&chats).unwrap();
        let cwd = root.join("working directory");
        fs::create_dir_all(&cwd).unwrap();
        fs::write(
            root.join("gemini/projects.json"),
            json!({"projects": {cwd.to_str().unwrap(): "project"}}).to_string(),
        )
        .unwrap();
        let records = [
            json!({"sessionId":"modern-id", "projectHash":"project", "startTime":"2026-10-17T10:00:00Z"}),
            json!({"id":"u1", "type":"user", "content":[{"text":"initial prompt"}]}),
            json!({"id":"a1", "type":"gemini", "content":"partial response"}),
            json!({"id":"a1", "type":"gemini", "content":"complete response"}),
            json!({"$patch":{"id":"u1", "content":[{"text":"edited prompt"}]}}),
            json!({"id":"u2", "type":"user", "content":"discarded prompt"}),
            json!({"$rewindTo":"u2"}),
            json!({"$set":{"lastUpdated":"2026-10-17T11:00:00Z"}}),
        ];
        let mut jsonl = records
            .iter()
            .map(Value::to_string)
            .collect::<Vec<_>>()
            .join("\n");
        jsonl.push_str("\n{\"incomplete\":");
        fs::write(chats.join("session-modern.jsonl"), jsonl).unwrap();
        fs::write(
            chats.join("session-legacy.json"),
            serde_json::to_string_pretty(&json!({
                "sessionId":"legacy-id", "lastUpdated":"2026-10-16T10:00:00Z",
                "messages":[{"type":"user", "content":"legacy prompt"}]
            }))
            .unwrap(),
        )
        .unwrap();

        let bin = root.join("bin");
        fs::create_dir_all(&bin).unwrap();
        let fake = bin.join("gemini");
        fs::write(&fake, "#!/bin/sh\nif [ \"$1\" = --list-sessions ]; then\n  echo '  2. edited prompt [modern-id]'\nelse\n  printf 'cwd: %s\nargv: %s %s\n' \"$PWD\" \"$1\" \"$2\"\n  exit 7\nfi\n").unwrap();
        fs::set_permissions(fake, fs::Permissions::from_mode(0o755)).unwrap();
        Self { root }
    }

    fn run(&self, args: &[&str]) -> Output {
        Command::new(env!("CARGO_BIN_EXE_ccr"))
            .args(args)
            .env("CCR_CLAUDE_DIR", self.root.join("no-claude"))
            .env("CCR_CODEX_DIR", self.root.join("no-codex"))
            .env("CCR_GEMINI_DIR", self.root.join("gemini"))
            .env(
                "PATH",
                format!(
                    "{}:{}",
                    self.root.join("bin").display(),
                    std::env::var("PATH").unwrap_or_default()
                ),
            )
            .output()
            .unwrap()
    }
}

impl Drop for Fixture {
    fn drop(&mut self) {
        fs::remove_dir_all(&self.root).ok();
    }
}

#[test]
fn lists_and_exports_both_formats() {
    let fx = Fixture::new("export");
    let list = fx.run(&["list"]);
    assert!(list.status.success());
    let text = String::from_utf8(list.stdout).unwrap();
    assert_eq!(text.lines().count(), 2);
    assert!(text.lines().next().unwrap().contains("modern-id"));
    assert!(text.contains("edited prompt"));
    assert!(text.contains("legacy-id"));
    assert!(!text.contains("discarded"));

    for (id, expected) in [
        ("modern-id", "edited prompt"),
        ("legacy-id", "legacy prompt"),
    ] {
        let output = fx.run(&["export", id, "--format", "json"]);
        assert!(output.status.success(), "{:?}", output);
        let doc: Value = serde_json::from_slice(&output.stdout).unwrap();
        assert_eq!(doc["turns"][0]["text"], expected);
        assert_eq!(
            doc["cwd"],
            fx.root.join("working directory").to_str().unwrap()
        );
        if id == "modern-id" {
            assert_eq!(doc["message_count"], 2);
            assert_eq!(doc["turns"][1]["text"], "complete response");
        }
    }
    let path = fx.run(&["path", "modern-id"]);
    assert!(path.status.success());
    assert!(
        String::from_utf8(path.stdout)
            .unwrap()
            .trim()
            .ends_with("session-modern.jsonl")
    );
}

#[test]
fn resumes_jsonl_session_in_its_project_directory() {
    let fx = Fixture::new("resume");
    let output = fx.run(&["resume", "modern-id"]);
    assert_eq!(output.status.code(), Some(7), "{:?}", output);
    let text = String::from_utf8(output.stdout).unwrap();
    assert!(text.contains("argv: --resume 2"));
    // macOS may expose the canonical /private/var prefix for temp directories.
    let cwd = text
        .lines()
        .find_map(|line| line.strip_prefix("cwd: "))
        .unwrap();
    assert_eq!(
        fs::canonicalize(cwd).unwrap(),
        fs::canonicalize(fx.root.join("working directory")).unwrap()
    );
}
