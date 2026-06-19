# Changelog

All notable changes to this project will be documented in this file.

## [Unreleased]

## [0.2.3] - 2026-06-19

### Bug Fixes

- *(stats)* Compute exact turn counts after tail-read scan
- *(export)* Emit exact message_count in json instead of null

### Documentation

- *(scan)* Document tail-window searchable scope

### Features

- *(scan)* Add read_tail byte-windowed file reader
- *(util)* Add file_mtime fallback for last_activity
- *(tui)* Compute detail-pane message count lazily

### Performance

- *(claude)* Scan only a tail window instead of full files
- *(codex)* Scan head meta + tail window instead of full files

### Refactoring

- *(session)* Make message_count Option for lazy counting
- *(scan)* Dedup tail-window constants and tidy scan loops
- *(scan)* Extract shared tail-window scan loop into tail::scan_windowed


## [0.2.2] - 2026-06-06

### Refactoring

- Use is_some_and for nickname filter match
Replace map(...).unwrap_or(false) with the clearer is_some_and idiom in the
  session filter predicate. No behavior change.


## [0.2.1](https://github.com/brevity1swos/ccr/compare/v0.2.0...v0.2.1) - 2026-05-23

### Added

- *(tui)* keep auto-title visible under nicknames (3-line layout)

### Other

- *(readme)* add demo gif with reproducible vhs recording

## [0.2.0](https://github.com/brevity1swos/ccr/compare/v0.1.1...v0.2.0) - 2026-05-23

### Added

- [**breaking**] trim ccr to its core picker — drop trash subsystem, add nicknames
- ccr stats — totals, per-tool, per-project, 30-day histogram
- *(export)* ccr export <id> [--format md|json]
- *(cli)* ccr path + ccr show — Unix-composable primitives

### Fixed

- move #[cfg(test)] mod tests to end of main.rs

### Other

- apply cargo fmt to v0.2.0 surface
- refresh README + CHANGELOG for the v0.2.0 surface

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
