# Changelog

All notable changes to this project will be documented in this file.

## [Unreleased]

## [0.1.0](https://github.com/brevity1swos/ccr/releases/tag/v0.1.0) - 2026-04-20

### Added

- *(backends)* add Codex and Gemini CLI backends
- soft-delete + prune-by-age with ~/.ccr/trash
- *(claude)* honor CLAUDE_CONFIG_DIR and CCR_CLAUDE_DIR
- initial ccr — CLI Code Resume TUI for Claude Code

### Fixed

- *(trash)* eliminate test race on CCR_TRASH_DIR

### Other

- prep for crates.io publish
- bump actions/checkout to v5 and pin rust-cache
- *(readme)* genericize examples and show full keybinding surface
- audit-fix sweep — techdebt, simplify, security
- apply simplify-pass review feedback
- add fmt/clippy/test/docs workflow and git-cliff config
- add unit tests for util helpers and Claude parser
- extract Backend trait for multi-tool support
