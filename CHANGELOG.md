# Changelog

All notable changes to this project will be documented in this file.

## [Unreleased]

## [0.1.1](https://github.com/brevity1swos/ccr/compare/v0.1.0...v0.1.1) - 2026-04-20

### Added

- ccr restore — move soft-deleted sessions back from trash
- full-turn content search via searchable blob
- *(tui)* v opens selected session in agx
- *(tui)* filter now scans preview turn content

### Fixed

- *(trash)* clippy sort_by_key on newer Rust stable

### Other

- *(scan)* parallelize per-file parsing with rayon
- *(readme)* center banner, pin CI badge to main branch
- *(readme)* post-publish polish
- release v0.1.0 ([#1](https://github.com/brevity1swos/ccr/pull/1))

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
