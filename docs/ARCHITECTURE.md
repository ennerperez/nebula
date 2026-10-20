# Architecture and SOLID Assessment

## Purpose

This document describes Nebula at `develop` commit `699f0f4`, identifies
refactoring pressure with source evidence, and defines a target architecture for
incremental SOLID adoption. It is an assessment, not a request to rewrite the
application. The public CLI, SQLite data, privacy guarantees, localization, and
portable single-binary distribution are compatibility boundaries.

## System context

Nebula is a Rust 2024 command-line application and agent skill. It receives
manual commands or host hook events, derives numeric communication metrics,
stores them in SQLite, and renders localized reports. Transcript text is read
only for in-memory analysis and must never be stored.

Runtime dependencies remain intentionally small: `clap`, `rusqlite`, `serde`,
`serde_json`, `chrono`, `regex`, `once_cell`, `dirs`, `wait-timeout`, and
`anyhow`. Locale catalogs are compiled into the executable. Report rendering,
including PDF, has no external runtime dependency. These constraints are
recorded in [STACK.md](STACK.md#runtime-considerations).

## Current architecture

The crate is a single binary with modules declared by the composition root in
[`src/main.rs`](../src/main.rs#L3). The effective layers are informal:

| Area | Current module | Responsibilities |
| --- | --- | --- |
| Entrypoint | `main.rs` | Parse CLI, open application state and database, resolve language, dispatch every command, adapt hook failures |
| CLI contract | `cli.rs` | `clap` command tree, value types, validation helpers |
| Runtime context | `app.rs` | Resolve home, open/initialize SQLite, read/write session JSON and language files, safely clear managed local data |
| Persistence | `db.rs` | Schema and views, seed catalog, metric/turn/keyword primitives |
| Text domain | `scoring.rs` | Model-score parsing, word/question/correction detection, fallback keywords, rule heuristics |
| Hook workflow | `hooks.rs` | Event routing, turn lifecycle, Git metadata, state recovery, transcript accounting, scoring, persistence, localized summary output |
| Manual workflow | `commands.rs` | `log-start`, `log-end`, and metric administration with SQL and console output |
| Analytics | `analytics.rs` | Filtering, SQL projections, aggregate analysis, typed report assembly, recommendation ingestion, numeric tip windows/gating, estimates, localized console output |
| Learning | `learning.rs` | Calibration queries, transcript rediscovery/reconstruction, feature extraction, ridge regression/CV, model persistence, heuristic replacement |
| Historical import | `import.rs` | Transcript discovery and Claude/Codex parsing, turn reconstruction, metadata discovery, scoring, transactional persistence, safe dump and score import |
| Host integration | `integrations.rs` | Safe, idempotent Claude Code settings merge, backup, and lifecycle hook configuration |
| Export | `export.rs` | Typed report model, five renderers, preferred/legacy destination policy, file naming and no-overwrite writes |
| Localization | `i18n.rs` | Language selection, embedded catalog validation, key lookup and interpolation |

### Primary data flows

1. **Command flow:** `Cli::parse` -> `App::new` -> `App::db` -> language
   resolution -> module-level command function -> direct SQLite access -> direct
   stdout/stderr output. See [`main`](../src/main.rs#L25) and
   [`resolve_language`](../src/main.rs#L153).
2. **Hook flow:** JSON stdin -> [`hooks::handle`](../src/hooks.rs#L35) -> session
   JSON plus transcript/Git inspection -> scoring and learned heuristic -> SQLite
   writes -> JSON host output. `user_prompt` and `stop` currently coordinate all
   stages ([`hooks.rs`](../src/hooks.rs#L53),
   [`hooks.rs`](../src/hooks.rs#L191)).
3. **Report flow:** SQLite view/filter query -> temporary `report_turns` table ->
   analytics queries -> localized typed `ReportDocument` -> selected renderer ->
   stdout or no-overwrite filesystem output. Agent-authored recommendations are
   validated before document assembly. See
   [`build_report`](../src/analytics.rs#L211),
   [`set_report_turns`](../src/analytics.rs#L593), and
   [`output_report`](../src/export.rs#L200).
4. **Import flow:** environment/default transcript roots -> recursive JSONL
   discovery -> provider-specific parsing -> privacy-safe turn reconstruction ->
   transaction per session -> optional safe dump. See
   [`import_with_roots`](../src/import.rs#L116),
   [`read_transcript`](../src/import.rs#L605), and
   [`import_transcript`](../src/import.rs#L201).
5. **Learning flow:** SQLite reviewed scores plus rediscovered transcript text ->
   feature vectors -> cross-validated ridge selection -> learned weights ->
   rescore heuristic values -> clear calibration. See
   [`fit_with_roots`](../src/learning.rs#L182) and
   [`learned_heuristic`](../src/learning.rs#L292).

### Persistence boundaries

[`db.rs`](../src/db.rs#L6) creates eight tables, four reporting views, and the
calibrated metric view. Schema setup is idempotent, but it is a single SQL string
that drops and recreates views at each database open. Domain concepts are mostly
represented as SQL rows, strings, tuples, and `BTreeMap<String, f64>` rather than
typed records. All major workflows accept a concrete `rusqlite::Connection`.

State outside SQLite is session-scoped JSON and language files under
`$NEBULA_HOME/state`, managed directly by [`App`](../src/app.rs#L7). Historical
transcripts remain external and are read on demand.

The destructive `data clear` path is dispatched before database initialization.
It removes only named Nebula-managed entries under `NEBULA_HOME`, preserving
the directory itself, unrelated files, and external transcript sources.

### Existing testing seams

- Database tests use `Connection::open_in_memory()` and real schema setup.
- Filesystem tests use isolated temporary directories and `App::for_test`.
- Import and learning have private `*_with_roots` functions for fixture roots.
- Pure scoring, feature, rendering, and CLI parsing functions have direct unit
  tests.

These seams provide useful regression coverage, but they are inconsistent and
private. Direct calls to `println!`, `std::env`, `dirs::home_dir`, `Utc::now`,
`std::fs`, and `Command::new("git")` still make orchestration tests stateful or
force tests to stop below the use-case boundary.

## Evidence-backed hotspots

The source contains approximately 8,322 Rust lines. Largest modules are
`analytics.rs` (1,717), `import.rs` (1,141), `learning.rs` (1,091), and
`export.rs` (1,057). A current AST dependency graph shows `ReportDocument`
already referenced by report assembly, five renderers, naming, and output
policy (15 connections); `output_report` coordinates 17 dependencies. These
figures are structural signals for decomposition, not quality scores.

### Hotspot catalog

| Hotspot | Evidence | Refactoring pressure |
| --- | --- | --- |
| Entrypoint dispatch | [`main`](../src/main.rs#L25) opens infrastructure before matching every `Cmd`; language resolution executes SQL at [`main.rs`](../src/main.rs#L153) | Composition, session lookup, localization policy, error policy, and dispatch change together |
| Runtime context | [`App`](../src/app.rs#L7) owns path policy, directory creation, SQLite construction, schema initialization, JSON state, and language files | One type is both configuration and multiple infrastructure adapters |
| Hook orchestration | [`user_prompt`](../src/hooks.rs#L53) and [`stop`](../src/hooks.rs#L191) combine payload parsing, recovery policy, Git/transcript I/O, calculations, DB writes, state transitions, and output | Hard to test failures or replace host-specific behavior independently |
| Analytics | [`build_report`](../src/analytics.rs#L211) coordinates queries and typed presentation data; [`filtered_turns`](../src/analytics.rs#L346) builds SQL; later sections calculate domain decisions and localized content in one module | Data access, analysis policy, localization, and presentation data remain coupled |
| Temporary report state | [`set_report_turns`](../src/analytics.rs#L256) mutates a connection-local temp table used by subsequent queries | Hidden ordering/state contract; repository calls cannot safely be reordered or parallelized |
| Import pipeline | [`import_with_roots`](../src/import.rs#L116) owns discovery, sorting, exclusion, idempotency, transactions, progress, dump collection; [`read_transcript`](../src/import.rs#L605) detects and parses two providers | New transcript providers or storage behavior require editing a large workflow module |
| Duplicate transcript infrastructure | Root discovery exists in [`import.rs`](../src/import.rs#L562) and [`learning.rs`](../src/learning.rs#L650); learning separately finds and parses messages at [`learning.rs`](../src/learning.rs#L751) | Behavior can drift between import and fit; privacy filtering is duplicated |
| Learning | [`fit_with_roots`](../src/learning.rs#L182) loads texts, extracts features, runs selection, writes weights, rewrites historical scores, resets calibration, and prints progress | Algorithm experiments and persistence policy cannot vary independently |
| Export | [`output_report`](../src/export.rs#L200) owns rendering dispatch and file policy; the same module defines the report model and implements Markdown/HTML/CSV/PDF | Adding a format changes central dispatch; model, renderer, and file-policy tests share module internals |
| Persistence | Schema plus seed in [`db.rs`](../src/db.rs#L6); use cases throughout `analytics`, `commands`, `hooks`, `import`, and `learning` execute SQL directly | SQL schema is effectively the application API; no repository contract or typed boundary |
| Output/localization | `Localizer` is passed through nearly every workflow; output is emitted inside use cases, for example [`analytics.rs`](../src/analytics.rs#L49) and [`learning.rs`](../src/learning.rs#L68) | Core results cannot be reused without presentation or captured uniformly in tests |
| Stringly typed domain | Direction/source/metric keys use values such as `"u2a"`, `"a2u"`, `"model"`, and `"heuristic"` across SQL and maps | Invalid states remain representable; compiler cannot enforce direction/source compatibility |

## SOLID assessment

### Single Responsibility Principle

Several leaf functions are focused, especially in `scoring.rs`, but module and
use-case boundaries are broad. `App`, hook handlers, import, learning, analytics,
and export each have multiple reasons to change. Highest-value split: separate
orchestration from domain calculation, persistence, external discovery, and
presentation.

### Open/Closed Principle

Extension requires central edits:

- a new export format changes `ExportFormat`, its extension match, and
  `output_report`/`render` dispatch;
- a new transcript source changes detection/parsing branches in import and likely
  learning;
- new command behavior changes the central `main` match;
- new metric behavior relies on string keys and SQL knowledge spread across
  modules.

Stable ports plus adapter registration can make transcript parsers, renderers,
and outputs extensible. The CLI enum should remain explicit because it is the
public contract; OCP does not justify hiding command definitions behind dynamic
registration.

### Liskov Substitution Principle

There are almost no behavioral interfaces, so substitution cannot currently be
verified. Introducing traits without contracts would only move coupling. Each
port needs explicit invariants: repository transaction semantics, state-store
missing/corrupt behavior, transcript privacy filtering, renderer determinism,
and file-writer no-overwrite behavior. Contract tests must run against in-memory
and production adapters where feasible.

### Interface Segregation Principle

Consumers currently receive concrete, capability-rich objects:
`rusqlite::Connection`, `App`, and `Localizer`. Narrow traits should follow use
cases, not mirror whole modules—for example `TurnReader`, `TurnWriter`,
`ModelStore`, `SessionStateStore`, `TranscriptCatalog`, `Clock`, and
`OutputSink`. Avoid one `NebulaRepository` god trait.

### Dependency Inversion Principle

Application/domain logic directly depends on `rusqlite`, filesystem layout,
environment variables, wall-clock time, Git subprocesses, and process output.
The desired dependency direction is:

```text
CLI / host payload adapters
           |
           v
application use cases ----> domain policies and value types
           |
           v
       narrow ports
           ^
           |
SQLite / JSON state / JSONL transcript / Git / filesystem / console adapters
```

Only the composition root constructs concrete adapters. Domain calculation must
not import `rusqlite`, `clap`, filesystem, environment, or console APIs.

## Target architecture

Use a pragmatic ports-and-adapters structure inside the existing binary crate:

```text
src/
  main.rs                  composition and process exit policy
  presentation/
    cli.rs                 stable clap contract and input mapping
    i18n.rs                localization adapter
    console.rs             stdout/stderr/host JSON sink
  application/
    turns.rs               start/end/hook use cases
    reports.rs             report/tips/estimate use cases
    imports.rs             historical import and score import
    learning.rs            calibration and model-fit orchestration
    metrics.rs             metric administration
  domain/
    model.rs               Direction, ScoreSource, MetricKey, Turn, filters
    scoring.rs             pure text scoring policies
    reporting.rs           typed report model and recommendations
    regression.rs          pure calibration/ridge algorithms
    transcript.rs          normalized transcript/message model
  ports/
    repository.rs          segregated read/write/model-store traits
    state.rs               session state and language contracts
    transcript.rs          discovery and parser contracts
    system.rs              clock and repository-metadata contracts
    output.rs              result/output and report-renderer contracts
  adapters/
    sqlite/                schema migrations and repository implementations
    state_json.rs          filesystem session state implementation
    transcript/            Claude and Codex JSONL parsers/catalog
    git.rs                 subprocess repository metadata
    report/                Markdown, HTML, JSON, CSV, PDF renderers
    filesystem.rs          atomic/no-overwrite report writer
```

Directory introduction should be incremental. Temporary compatibility facades
may preserve current module paths while callers migrate.

### Core value types

Introduce types only where they eliminate repeated validation or string
protocols:

- `Direction::{UserToAgent, AgentToUser}`
- `ScoreSource::{Model, Heuristic, Hook, Transcript}`
- `MetricKey` with validated built-in and custom forms
- `ReportFilter`, `TurnId`, `SessionId`, `Outcome`
- normalized `Transcript`, `Message`, `Role`, and `TranscriptProvider`
- move existing typed `ReportDocument`/`ReportSection` out of the export adapter
  and keep it independent of every renderer

Conversion to database strings and CLI spellings belongs at adapter boundaries.

### Port design constraints

- Prefer several small traits over a single repository abstraction.
- Accept `&dyn Trait` at coarse use-case boundaries; keep pure algorithms generic
  over iterators/data, avoiding unnecessary dynamic dispatch.
- Preserve transaction ownership in application workflows. A unit of work must
  make session import and model adoption atomic.
- Return structured results from use cases. Presentation adapters localize and
  print them.
- Keep privacy filtering before any persistence port receives data. Repository
  input types must not contain transcript text.
- Keep SQLite views and public output unchanged until characterization tests
  prove equivalence.

## Compatibility and quality constraints

1. CLI command names, arguments, defaults, exit behavior, and English/Spanish
   output remain compatible unless explicitly versioned.
2. Existing SQLite databases open without loss. Use ordered, transactional,
   idempotent migrations; never rebuild user tables destructively.
3. Prompt, response, rationale, source code, credentials, and token content never
   cross the persistence boundary.
4. Hook errors continue to report to stderr and return success so instrumentation
   cannot break host sessions.
5. Reports preserve filters and MD/HTML/JSON/CSV/PDF behavior, including
   structured cross-format semantics, agent-supplied recommendations,
   preferred/legacy destinations, no-overwrite creation, and spreadsheet
   formula neutralization.
6. Tips remain agent-authored, transient, limited to 90 characters, and gated
   by same-session numeric history; application code must not add a prose
   catalog.
7. Rust 1.85 and the small, self-contained release remain supported.
8. Unit tests stay above the enforced 85% line and branch coverage thresholds.

## Refactoring strategy

Proceed by characterization, extraction, and dependency inversion—not a rewrite.
First lock external behavior and introduce typed domain values. Then extract
shared transcript and reporting models. Add narrow ports at tested seams, move
SQLite/filesystem/Git implementations behind them, and migrate one use case at a
time. Delete compatibility facades only after all callers and tests have moved.

The executable must remain buildable and unit-testable after every phase. The
detailed waves, dependencies, acceptance criteria, tests, risks, worktrees, and
commit messages are in `docs/plan-NEBULA-solid-refactor.md` (intentionally
ignored by Git as a working plan).
