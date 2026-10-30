# Nebula CLI guide

## What Nebula does

Nebula measures communication and execution efficiency for AI-agent turns.
Its hooks and agent-facing commands cooperate to record:

- estimated and actual elapsed time;
- input and output word counts;
- task type, outcome, short keywords, and correction counts;
- numeric user-to-agent (`u2a`) and agent-to-user (`a2u`) clarity scores;
- aggregate "nebulosity" derived from ten clarity dimensions.

Data stays in a local SQLite database. Nebula processes hook prompt and
response text in memory to obtain counts and heuristics, but does not store the
text, rationale, source code, credentials, or token content. The same invariant
applies to transcript import and learned heuristics. An import dump contains
safe metadata and a scoring scaffold, never prompt or answer text.

Nebula provides turn hooks, explicit logging, localized reports and exports,
historical estimates and tips, heuristic learning and calibration, historical
import, and metric-definition management.

## Installation

### Requirements

The POSIX release installer requires:

- a POSIX shell;
- `curl` or `wget`;
- `sha256sum`, `shasum`, or `openssl` for checksum verification;
- `tar` for Linux and macOS archives, or `unzip` for Windows archives.

The native Windows installer requires Windows PowerShell 5.1 or PowerShell 7.
Published release packages cover Linux, macOS, and Windows on `x86_64` and
`aarch64`. The POSIX installer also runs on Windows under Git Bash, MSYS2, or
Cygwin.

Release CI validates the selected `vX.Y.Z` tag and injects `X.Y.Z` as the
compile-time CLI version. Local builds without that release variable report the
package version from `Cargo.toml`.

| Platform | Architecture | Release asset |
| --- | --- | --- |
| Linux | `x86_64` | `nebula-linux-x86_64.tar.gz` |
| Linux | `aarch64` | `nebula-linux-aarch64.tar.gz` |
| macOS | `x86_64` | `nebula-macos-x86_64.tar.gz` |
| macOS | `aarch64` | `nebula-macos-aarch64.tar.gz` |
| Windows | `x86_64` | `nebula-windows-x86_64.zip` |
| Windows | `aarch64` | `nebula-windows-aarch64.zip` |

### Verified release installer

On Linux and macOS, download and inspect the script, then run it:

```sh
curl --proto '=https' --tlsv1.2 -fsSLO \
  https://raw.githubusercontent.com/ennerperez/nebula/develop/install.sh
sh install.sh
```

On Windows, use native PowerShell:

```powershell
irm https://raw.githubusercontent.com/ennerperez/nebula/master/install.ps1 | iex
```

The PowerShell one-liner installs only the CLI. To install an agent integration,
download the script and pass its name:

```powershell
irm https://raw.githubusercontent.com/ennerperez/nebula/master/install.ps1 -OutFile install.ps1
.\install.ps1 -NoCli -Agent codex
```

Both installers:

1. resolve the latest release for `ennerperez/nebula`;
2. choose an archive for the detected operating system and architecture;
3. download the archive and `SHA256SUMS` into a temporary directory;
4. verify the checksum and reject unsafe archive paths before extraction;
5. install the CLI to `~/.local/bin` and verify `nebula --version`;
6. copy the bundled Nebula skill to selected agent roots only;
7. configure `UserPromptSubmit`, `Stop`, and permission-wait hooks when Codex
   or Claude Code is selected;
8. add `<CODEX_HOME>/nebula` to Codex
   `sandbox_workspace_write.writable_roots` when Codex is selected.

Both scripts verify checksums before extraction, then run the installed Nebula
binary to verify its version and configure supported host hooks. If `~/.local/bin` is
not on `PATH`, it prints a reminder after installation.

Installer options:

```text
--version <version>    Release tag, such as v0.1.0 (default: latest)
--repo <owner/repo>    GitHub repository (default: ennerperez/nebula)
--bin-dir <path>       CLI destination (default: ~/.local/bin)
--agent <agent>        auto, all, codex, claude, gemini, opencode, generic,
                       or none; default: none; may be repeated
--skill-dir <path>     Additional skills root; may be repeated
--no-cli               Install only the skill
--no-skill             Install only the CLI
--no-hooks             Do not configure Codex or Claude Code lifecycle hooks
--repair-hooks         Repair selected hooks with installed CLI; no download
-h, --help             Print installer help
```

Equivalent environment overrides are `NEBULA_VERSION`, `NEBULA_REPO`,
`NEBULA_INSTALL_DIR`, and `NEBULA_AGENT`.

PowerShell uses `-Version`, `-Repo`, `-BinDir`, `-Agent`, `-SkillDir`, `-NoCli`,
`-NoHooks`, and `-RepairHooks`. Pass multiple agent names as `-Agent codex,claude`. It uses the
same environment overrides and Windows default binary directory
`$HOME\.local\bin`.

Examples:

```sh
# Install a specific release to a custom binary directory.
sh install.sh --version v0.1.0 --bin-dir "$HOME/bin"

# Install only the CLI.
sh install.sh --no-skill

# Install only the skill for all known agents.
sh install.sh --no-cli --agent all

# Install the skill for an Agent Skills-compatible custom agent.
sh install.sh --no-cli --skill-dir "$HOME/my-agent/skills"

# Install Claude integration in a custom config and binary location.
CLAUDE_CONFIG_DIR="$HOME/.claude2" \
  sh install.sh --agent claude --bin-dir "$HOME/bin"
```

### Agent detection and skill locations

Agent installation is opt-in. In `auto` mode, Nebula detects an agent when its
executable or configuration directory exists. It installs the skill to:

| Agent | Skill directory | Default data directory |
| --- | --- | --- |
| Codex | `${CODEX_HOME:-~/.codex}/skills/nebula` | `${CODEX_HOME:-~/.codex}/nebula` |
| Claude Code | `${CLAUDE_CONFIG_DIR:-~/.claude}/skills/nebula` | `${CLAUDE_CONFIG_DIR:-~/.claude}/nebula` |
| Gemini CLI | `${GEMINI_CLI_HOME:-~}/.gemini/skills/nebula` | `${GEMINI_CLI_HOME:-~}/.gemini/nebula` |
| OpenCode | `${XDG_CONFIG_HOME:-~/.config}/opencode/skills/nebula` | `${XDG_CONFIG_HOME:-~/.config}/opencode/nebula` |
| Generic fallback | `~/.agents/skills/nebula` | `~/.agents/nebula` |

When no supported agent is detected in `auto` mode, installer uses generic location.
`--agent all` targets every known agent location. `--agent none` disables skill
installation unless explicit `--skill-dir` targets were also supplied.

For Codex, the installer merges hooks into `${CODEX_HOME:-~/.codex}/hooks.json`,
preserves unrelated entries, and creates `hooks.json.bak-nebula` before its
first change. Restart Codex, then use `/hooks` to review and trust the exact
installed definitions; Codex intentionally skips untrusted hooks.

For Claude Code, the installer merges hooks into
`${CLAUDE_CONFIG_DIR:-~/.claude}/settings.json`, preserves unrelated settings
and hooks, and creates `settings.json.bak-nebula` before its first change. Hook
commands use the absolute installed binary path, so custom `--bin-dir` values do
not depend on Claude's `PATH`. Repeated installs are idempotent. Use
`--no-hooks` to opt out. A skill-only install reuses `nebula` from `PATH`; if no
binary is available, it leaves hooks unchanged and prints a notice.
Restart the host after the installer changes hook configuration; recording
starts with the next submitted prompt after any required trust review.

To repair existing hooks without downloading a release, use the installed CLI:

```sh
sh install.sh --repair-hooks --agent codex --agent claude
```

On Windows, use `./install.ps1 -RepairHooks -Agent codex,claude`. Windows
installation and repair rewrite legacy POSIX Nebula hook commands into native
Windows commands, even when the installed CLI still emits the older format.
Both repair modes require an installed Nebula CLI and leave unrelated hooks intact.

Other hosts must invoke `nebula hook` at prompt, stop, and notification events
where supported.

### Install from a release archive

Release archives contain platform binary, `README.md`, `LICENSE`, `docs`,
embedded locale catalog sources under `locales`, and `skills/nebula`.
Download matching archive and `SHA256SUMS` from repository's GitHub Releases
page, verify archive, then place binary on `PATH` and copy skill directory to an
agent location listed above. Locale strings are compiled into the executable;
the binary needs no adjacent runtime catalog files.

### Build from source

Install Rust 1.85 or newer, clone repository, then run:

```sh
cargo build --release --locked --bin nebula
```

Executable is written to `target/release/nebula` (or
`target/release/nebula.exe` on Windows). To install binary through Cargo:

```sh
cargo install --locked --path .
```

Copy `skills/nebula` manually when a source installation also needs Agent
Skill.

### Update and uninstall

Nebula has no built-in update or uninstall command. Re-run `install.sh` or
`install.ps1` to replace CLI and skill with latest or selected release.

To uninstall, remove `nebula` executable from selected binary directory and
remove `nebula` from each skills directory where installed. Metrics remain
intact unless you run `nebula data clear --yes` first.

## Storage and configuration

Agent skill calls pass their installation root with `--agent-home`. Generated
hooks may use the equivalent internal environment variable
`NEBULA_AGENT_HOME`. Nebula appends `nebula`, giving every agent installation an
independent data directory. For example, Codex stores:

```text
~/.codex/nebula/
├── metrics.db
├── reports/
└── state/
```

Standalone CLI calls without agent context use `~/.agents/nebula`. For a custom
installation at `<agent-home>/skills/nebula`, pass
`--agent-home <agent-home>`. When the original Python implementation's
`<agent-home>/nebular/metrics.db` exists and the new directory does not, Nebula
reuses that database in place.

`reports/` is created on demand for preferred HTML and PDF output. Other
preferred formats use stdout unless `--output` is given.

Permanently remove all paths managed inside this directory:

```sh
nebula data clear --yes
```

This deletes `metrics.db` and its SQLite sidecars, `state/`, and `reports/` from
the effective Nebula home. It leaves that directory and unrelated files intact.
It never removes Claude/Codex transcripts, exports written outside those
managed paths, the Nebula executable, or installed skills. The command refuses
to run without `--yes` and rejects an effective home resolving to a filesystem
root.

Set `NEBULA_HOME` explicitly when agents should use one shared location. It has
priority over `--agent-home` and `NEBULA_AGENT_HOME`:

```sh
NEBULA_HOME="$HOME/.local/share/nebula" nebula report
```

`NEBULAR_HOME` remains a deprecated compatibility alias for the original Python
skill. `NEBULA_HOME` wins when both exist.

Database schema is created and seeded on first command execution, including
commands such as `report` and `metrics list`.

### Language selection

Commands that produce user-facing output accept `--lang en` or `--lang es`.
Resolution order is explicit option, current session language, `NEBULA_LANG`,
deprecated `NEBULAR_LANG`, then English. An explicit language on a session
command is persisted under `state/` for later hook output. Catalogs are complete
and embedded; unknown language codes fail instead of mixing languages or
silently falling back.

## Turn lifecycle

Normal operation follows this order:

1. Host sends `user-prompt-submit`; Nebula opens a turn, counts prompt words,
   and records heuristic `u2a` values.
2. Agent runs `log-start`; Nebula attaches estimate, keywords, optional task
   metadata, and model-provided `u2a` scores to that open turn.
3. Agent completes work and runs `tips` to compare the current prompt with its
   recent same-session baseline.
4. Agent writes one live tip only when that gate is on, then runs `log-end`;
   Nebula records model-provided `a2u` scores, optional outcome, and tip.
5. Host sends `stop`; Nebula records actual elapsed time, estimate ratio,
   response word count, heuristic `a2u` values, request/agent activity, and
   closes turn.

Intermediate stops refresh metrics without closing a turn that has not reached
`log-end`. If `log-end` never arrives, the next prompt recovers the previous
turn at its last stop before opening the next one. Internal subagent messages
are ignored as user prompts. Where host payloads provide the data, Nebula also
captures repository, branch, ticket, model, blocked time, and subagent
transcript activity.

Example manual lifecycle:

```sh
SCORES='{"ambiguity":10,"idea_branching":10,"idea_dispersion":10,"context_gap":10,"explanation_gap":10,"argument_gap":10,"lucidity_deficit":10,"structural_complexity":10,"opacity":10,"certainty_deficit":10}'

printf '%s' '{"session_id":"demo","prompt":"Explain the release process"}' \
  | nebula hook user-prompt-submit

nebula log-start --session demo --estimate 120 \
  --keywords 'release,documentation' --type explain --u2a "$SCORES"

nebula tips --session demo
nebula log-end --session demo --outcome completed --a2u "$SCORES"

printf '%s' '{"session_id":"demo","last_assistant_message":"Release explained."}' \
  | nebula hook stop
```

Each score object must contain all ten documented keys. Values are numeric and
clamped to range 0 through 100.

## Command reference

Run `nebula help <COMMAND>` for generated help. Global options are
`--agent-home <PATH>`, `-h`/`--help`, and `-V`/`--version`.

### `nebula configure claude`

Merges Nebula's three lifecycle hooks into an existing Claude Code settings
file without removing unrelated settings or hook commands:

```sh
nebula configure claude \
  --settings "$CLAUDE_CONFIG_DIR/settings.json" \
  --binary "$HOME/bin/nebula"
```

The command accepts a missing settings file, rejects malformed JSON without
changing it, creates a one-time `.bak-nebula` backup, replaces legacy bare
`nebula hook ...` entries, removes duplicate Nebula entries, and is idempotent.

### `nebula configure codex`

Merges Nebula's Codex lifecycle hooks into the user-level hooks file without
removing unrelated definitions:

```sh
nebula configure codex \
  --hooks "$CODEX_HOME/hooks.json" \
  --binary "$HOME/bin/nebula"
```

It installs `UserPromptSubmit` and `Stop` directly. Codex
`PermissionRequest` is mapped to Nebula's notification event for wait-time
tracking. Missing files are created, malformed JSON is rejected unchanged,
the first replacement creates a `.bak-nebula` backup, and repeated runs are
idempotent. See the [official Codex hooks documentation](https://developers.openai.com/codex/hooks).

### `nebula data clear`

```text
nebula data clear --yes [--lang <en|es>]
```

Permanently deletes all Nebula-managed database, transient state, and generated
report paths under `NEBULA_HOME`. This command dispatches before database
initialization, so clearing an empty location does not recreate `metrics.db`.
The `--yes` flag is mandatory to prevent accidental deletion.

### `nebula hook <EVENT>`

Reads one host event as JSON from standard input.

Events:

- `user-prompt-submit` opens turn and emits JSON containing additional agent
  context.
- `stop` finalizes a model-closed turn and emits compact `systemMessage`.
- `notification` records the start of permission waits when the host payload
  identifies a permission prompt; other notification types are ignored.

Hook-processing errors are printed as `nebula hook error: ...` without failing
host event. Initialization errors occurring before event processing still
return non-zero exit status.

### `nebula log-start`

```text
nebula log-start --estimate <SECONDS> --keywords <CSV> --u2a <JSON>
                  [--type <TYPE>] [--clarifying <COUNT>]
                  [--correction <0|1>] [--session <ID>] [--lang <en|es>]
```

Records agent estimates and model scores on existing open turn. Keywords are
trimmed, converted to lowercase, and stored individually. `--correction` is
applied to preceding turn when one exists. Session resolution order: explicit `--session`, `CLAUDE_CODE_SESSION_ID` when set, then most recently started open turn. This keeps Claude Code commands scoped to the current session.

`--estimate` must be a finite number greater than zero and `--clarifying`
must be a non-negative integer.

Success output resembles `turn 2 logged`. It fails when no open turn exists or
when `--u2a` is invalid JSON or omits a required score.

### `nebula log-end`

```text
nebula log-end --a2u <JSON> [--outcome <LABEL>] [--tip <TEXT>]
                [--session <ID>] [--lang <en|es>]
```

Records model `a2u` scores for open turn and marks model-side work complete. Uses the same session resolution order as `log-start`.
Tips must be agent-written, direct actions of at most 90 characters. Overlong
tips fail and are never truncated. They remain in transient session state, not
the metrics database. The stop hook displays a tip only when the current
prompt's `u2a` is worse than its recent same-session baseline; otherwise it
discards the tip. Run `tips` immediately before `log-end` to apply that gate.
Host `stop` event performs final timing and closure.

Success output resembles `turn 2 closed by model`. Error conditions match
`log-start` for open-turn selection and score parsing.

### `nebula analyze`

```text
nebula analyze (--text <TEXT> | --file <PATH>)
               [--direction <u2a|a2u>] [--lang <en|es>]
```

Analyzes all supplied text with Nebula's rule-based heuristic and prints the
weighted nebulosity score plus its ten indices. Direction defaults to `u2a`;
use `a2u` for assistant text. File input reads the complete UTF-8 file.
This command does not initialize the database, save input, or save results.
It does not use model scores or learned calibration.

```sh
nebula analyze --text "Please explain this error and suggest a fix."
nebula analyze --file ./prompt.txt --direction a2u --lang es
```

### `nebula report`

```text
nebula report [u2a|a2u] [--days <DAYS>] [--keyword <KEYWORD>]
               [--repo <REPO>] [--recs <JSON_OR_FILE>]
               [--format <md|html|json|csv|pdf>] [-o|--output <PATH>]
               [--export <md|html|json|csv|pdf>] [--lang <en|es>]
```

Without a format option, the command prints the complete Markdown report to
stdout. Scope `u2a` or `a2u` limits profiles and recommendations to that
direction. `--days`, `--keyword`, and `--repo` restrict every report section to
matching turns. With no rows, output is `no data`.

`--format` is the preferred interface. Markdown, structured JSON, and tidy CSV
go to stdout by default. Standalone HTML and portable PDF always create local
files, defaulting to safe timestamped names under
`<effective-nebula-home>/reports`. `-o`/`--output` selects an exact destination
for any format; with no format it writes Markdown. HTML is self-contained with
light/dark styling. PDF is generated without a browser or network. CSV uses:

```text
section,direction,item,label,measure,value,text
```

Fields that begin with spreadsheet formula prefixes are neutralized.

`--export` is a compatibility alias. It writes every format to a timestamped
file in the current directory when `--output` is omitted. Supplying different
values to `--format` and `--export` fails. Explicit and generated paths never
overwrite existing files.

Recommendations are written by the agent, not synthesized by the CLI. Run
`nebula tips --both`, derive concrete actions, and pass inline JSON or a local
JSON file through `--recs`:

```json
{"you":["Use one exact target"],"me":["Lead with the result"],"both":["Confirm scope early"]}
```

Without `--recs`, the report contains a localized note that recommendations
were not supplied. Direction-scoped reports emit only relevant groups; SWOT
remains computed from numeric rules.

```sh
nebula report --days 30 --format html
nebula report u2a --keyword rust --format json -o u2a-rust.json
nebula report --repo owner/nebula --format pdf --lang es
nebula report --format md --recs recommendations.json
nebula report --export html # compatibility: writes in current directory
```

### `nebula metrics list`

```text
nebula metrics [--lang <en|es>] list
```

Lists metric key, category, unit, whether lower or higher values are better,
parent, weight, and directional flag. Initial catalog contains core execution
metrics, aggregate `nebulosity` metric, and all ten clarity dimensions. Stable
database keys remain unchanged across languages.

### `nebula metrics add`

```text
nebula metrics [--lang <en|es>] add <KEY> <NAME> <CATEGORY> <UNIT> <BETTER>
                    [--parent <KEY>] [--weight <NUMBER>]
                    [--aggregation <NAME>] [--directional]
```

Adds metric definition. `<BETTER>` must be `lower` or `higher`. Default weight
is `1` and must be finite and non-negative; default aggregation is `sum`.
Parent key must already exist. Nebula reports `metric <KEY> added` on success
and returns database error for duplicate key.

### `nebula tips`

```text
nebula tips [--share <NUMBER>] [--both] [--all]
             [--session <ID>] [--lang <en|es>]
```

Prints numeric evidence for live agent-written guidance; it does not generate
advice. The first line reports whether the current prompt's `u2a` is worse than
the recent same-session baseline (`gate_on`) or not (`gate_off`). For each
sub-metric, output includes recent window, earlier baseline, delta, and last
turn.

Default window is the latest 30% of scored turns in the selected conversation.
`--share` accepts a finite value and is clamped to `0.3..1.0`; `--both` includes
`a2u`; `--all` uses scored turns across all sessions; `--session` selects a
known session rather than the latest active/recent one. Use the output to write
one direct, session-language tip of at most 90 characters only while gate is
on.

### `nebula estimate`

```text
nebula estimate --keywords <CSV> [--lang <en|es>]
```

Matches normalized keywords against historical turns and prints match count,
weighted estimate, median, p90, and prior estimate ratio when available. It
reports that no history matches instead of inventing an estimate.

### `nebula calibrate`

```text
nebula calibrate [--min-samples <COUNT>] [--lang <en|es>]
```

Fits a least-squares mapping from heuristic to model values for every clarity
dimension and direction. Default minimum is 20 paired samples. Raw heuristic
values remain unchanged; reporting views apply calibration when read. The
minimum sample count must be at least one.

### `nebula fit`

```text
nebula fit [--min-samples <COUNT>] [--dry-run] [--lang <en|es>]
```

Reconstructs message text from locally available supported transcripts in
memory, extracts linguistic features, and evaluates ridge models with five-fold
cross-validation. Default minimum is 40 samples. A model is adopted only when
its validation correlation clears the quality threshold and beats the existing
rule. A non-dry run recomputes heuristic values for matched historical turns
and resets calibration; transcript text and feature vectors are not persisted.
The minimum sample count must be at least one.

### `nebula import`

```text
nebula import [--sessions <COUNT>] [--exclude <ID> ...] [--host <claude|codex>]
               [--dump <PATH>] [--lang <en|es>]
```

Discovers recent Claude-compatible project transcripts and Codex session
transcripts recursively. Inside Codex, discovery is limited to
`${CODEX_HOME:-~/.codex}/sessions`. Inside Claude Code, it is limited to the
active `${CLAUDE_CONFIG_DIR:-~/.claude}/projects`; alternate profiles do not
read standard Claude or Codex history. Agent integrations should always pass
`--host claude` or `--host codex`; explicit identity also works when subprocess
environment markers are unavailable. Ambiguous or unknown hosts fail closed
instead of combining roots. Set `NEBULA_TRANSCRIPTS` to an operating-system path
list to explicitly select transcripts when host detection is unavailable.
Excluded or already imported sessions are skipped. Default limit is 10 sessions.
Claude subagent activity is folded into its parent session; subagent transcript
files are not imported as separate sessions. The session limit must be at least
one.

Text is processed only in memory. When requested, `--dump` writes schema
`nebula.safe-import-dump.v1` with safe turn/session identifiers, timestamps,
word/request/agent counts, keywords, and heuristic scores; it never includes
prompt or answer content. Treat the dump as local workflow data and remove it
when no longer needed.

### `nebula import-scores`

```text
nebula import-scores <FILE> [--lang <en|es>]
```

Reads a JSON array whose rows contain `turn_id` and may contain complete `u2a`
or `a2u` score objects, `keywords`, `task_type`, `clarifying_questions`, and
`corrects_previous`. It validates scores like `log-start`/`log-end`, updates
known imported turns, and skips unknown turn IDs.

## Report contents and schema

The full report contains an introduction and scale, execution/usage summary,
average, minimum, median, and maximum turns per session, first-half versus
second-half trends, model-versus-heuristic bias, keyword analysis, a profile for
each selected direction, rule-based SWOT analysis, and agent-supplied
recommendations for user, agent, and both sides. Without `--recs`, it records
that recommendations are missing. Direction-specific reports omit unrelated
profiles and recommendation groups.

One typed report model feeds Markdown, HTML, JSON, CSV, and PDF so filters,
scope, labels, measurements, and recommendations stay consistent across
formats.

Core tables are `sessions`, `turns`, `keywords`, `turn_keywords`,
`metric_definitions`, and `metric_values`. Learned ridge coefficients and
linear calibration are stored separately. `v_turn_summary`, `v_nebulosity`,
`v_keyword_stats`, and `v_bias` expose reporting data without altering raw
metric values. Schema setup is additive so existing databases retain history.

## Errors and troubleshooting

- `no open turn ...`: ensure `user-prompt-submit` hook opened same session
  before `log-start` or `log-end`.
- `--u2a must be JSON` or `--a2u must be JSON`: pass one valid JSON object as
  single shell argument.
- `--u2a missing: <key>` or `--a2u missing: <key>`: include all ten clarity
  dimensions.
- Unknown language: use `en` or `es`, or correct `NEBULA_LANG`.
- Fit/import finds no transcript samples: confirm supported host history exists
  in its standard or configured home and that session identifiers match.
- Recommendation JSON fails: pass an object containing only `you`, `me`, and
  `both` string arrays, or a readable local file containing that object.
- Tip is rejected: keep it to one direct action of 90 characters or fewer.
- Export path exists: choose another `--output`; Nebula never overwrites it.
- Installer checksum errors: do not bypass verification; confirm archive and
  `SHA256SUMS` come from same release.
- `nebula: command not found`: add selected `--bin-dir` to `PATH`.

For machine-generated syntax, use:

```sh
nebula --help
nebula help log-start
nebula metrics add --help
sh install.sh --help
```
