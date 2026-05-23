# Changelog

All notable changes to this project will be documented in this file.

## [Unreleased]

> **Breaking change.** The soft-delete / prune / restore subsystem has been
> removed entirely. If you have files in `~/.ccr/trash/` that you still care
> about, restore them with v0.1.1 before upgrading. Otherwise the directory
> can be removed by hand (`rm -rf ~/.ccr/trash`).

### Added
- *(tui)* session nicknames — press `n` on any row to set, edit, or clear a
  custom yellow label that replaces the auto-derived title in the list.
  Persisted in `~/.ccr/nicknames.json` (override with `CCR_NICKNAMES_FILE`).
- *(tui)* nickname matching in the `/` filter; nickname also shown in the
  preview pane as a `nick:` line.

### Changed
- *(backends)* session title is now the **last** user message in the session,
  not the first — better reflects "what was I working on most recently."
- *(scan)* the three backends now run in parallel via rayon
  (`par_iter` at the backend level). Per-file parsing was already parallel.

### Removed
- `ccr prune` and `ccr restore` subcommands.
- TUI keys `d` (soft-delete) and `D` (prune-by-age).
- `Backend::trash()` trait method and its default implementation.
- `auto_prune()` on startup.
- `src/trash.rs` and `src/age.rs` modules.
- `CCR_TRASH_DIR` environment variable.

### Fixed
- Test race between `bookmarks` and `nicknames` modules — both now factor
  out path-accepting inner functions (`save_to` / `load_from`) so tests
  bypass the shared process env vars entirely.

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
