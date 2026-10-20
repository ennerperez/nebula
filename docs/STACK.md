# Stack

- Language: Rust 2024 edition, minimum supported Rust version 1.85.
- CLI parsing: `clap` derive API.
- SQLite: `rusqlite` with bundled SQLite for portable builds.
- Serialization: `serde` and `serde_json`.
- Date/time: `chrono` with UTC timestamps.
- Text processing: `regex` and `once_cell`.
- Error handling: `anyhow`.
- Test support: Rust unit tests with isolated temporary state and in-memory SQLite databases.
- Coverage: `cargo-llvm-cov` on nightly Rust with LLVM branch instrumentation; `scripts/check-coverage.sh` enforces at least 85% line and branch coverage over production code (test-module instrumentation is excluded).
- CI: GitHub Actions builds every push to `develop`; pull requests build and run coverage-gated unit tests.

## Runtime considerations

- Default data directory is `~/.nebula`; `NEBULA_HOME` overrides it.
- Database schema and command output remain compatible with Python Nebula where practical.
- Prompt, response, rationale, and token text must never be persisted in SQLite. Transcript text is processed only in memory.
- Import dumps contain only safe identifiers, metadata, and a scoring scaffold;
  prompt and response content is excluded.
- Hook failures print an error to stderr and exit successfully so metrics cannot break host sessions.
- Tips are agent-authored transient text, limited to 90 characters and gated by
  current-versus-recent same-session `u2a`; no automatic tip catalog is shipped.
- English and Spanish catalogs are compiled into the binary with `include_str!`;
  no external locale lookup is required at runtime.
- Reports are assembled as a typed document before rendering. Agent-written
  recommendations enter through validated JSON or a local JSON file; report
  renderers never synthesize recommendation prose.
- Local coverage requires nightly Rust, `llvm-tools-preview`, and `cargo-llvm-cov`; run `./scripts/check-coverage.sh`.

## Source architecture

- `src/main.rs`: composition root and command dispatch only.
- `src/cli.rs`: stable `clap` command and argument definitions.
- `src/app.rs`: application home, SQLite opening, and JSON state files.
- `src/db.rs`: schema, seed data, and persistence primitives.
- `src/scoring.rs`: score parsing, word counting, and text heuristics.
- `src/hooks.rs`: host event lifecycle, transcript activity, recovery, and stop summaries.
- `src/commands.rs`: log and metric administration handlers.
- `src/analytics.rs`: structured report assembly, recommendation ingestion,
  trends, bias, keyword analysis, numeric tip windows/gates, and estimates.
- `src/learning.rs`: transcript reconstruction, feature extraction, ridge fitting,
  and heuristic calibration.
- `src/import.rs`: Claude/Codex transcript discovery, privacy-safe historical
  reconstruction, safe dump serialization, and reviewed-score import.
- `src/export.rs`: typed report model, local Markdown, standalone HTML,
  structured JSON, tidy CSV, and dependency-free PDF serialization with
  preferred/legacy destination policy and no-overwrite file creation.

Report exports use only Rust standard-library I/O plus existing `serde_json`
and `chrono` dependencies. PDF output is generated directly as PDF 1.4 with
built-in Helvetica/WinAnsi resources, so no browser, office suite, or external
process is required at runtime. Preferred HTML/PDF output defaults to
`$NEBULA_HOME/reports`; preferred Markdown/JSON/CSV defaults to stdout.
`--export` preserves the legacy current-directory file behavior.

Release archives include the executable, license, README, all project docs,
English/Spanish catalog sources, and the complete `skills/nebula` bundle,
including host metadata. Runtime localization and report export remain
self-contained inside the executable; catalogs in the archive are
reference/source assets.

