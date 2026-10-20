---
name: nebula
description: Record per-turn communication and efficiency metrics with the local Nebula Rust CLI. Use every turn while Nebula is active, and for reports, estimates, tips, calibration, or communication-efficiency questions.
---

# Nebula

Run `nebula` from `PATH`. Nebula stores its database and transient state under
`~/.nebula`; honor `NEBULA_HOME` when already set. The release installer can
place this skill in standard Codex, Claude Code, Gemini CLI, OpenCode, and
generic Agent Skills roots, or in explicit skill roots. Skill installation does
not configure host hooks automatically.

## Privacy invariant

Never persist prompt text, response text, rationale, source code, credentials,
or token content. Only numeric values, short lowercase keywords, and documented
categorical labels belong in SQLite. Transcript import and learning may process
text in memory. Safe import dumps contain metadata and a scoring scaffold only;
they must never contain prompt or answer text.

## Every active turn

The host `user-prompt-submit` hook opens a turn and injects
`[nebula] turn N open`.

1. Before task tools, record the turn. Use `nebula estimate --keywords ...`
   first when history can improve the estimate; otherwise use an honest
   wall-clock estimate.

   ```sh
   nebula log-start --estimate <seconds> --keywords k1,k2,k3 \
     --type <plan|code|debug|review|explain|shell|search|design|chat> \
     [--clarifying <count>] [--correction 1] \
     --u2a '<scores-json>' --lang <en|es>
   ```

2. Complete the user's task.
3. Immediately before the final response, inspect the live conversation data,
   then close model-side work:

   ```sh
   nebula tips [--session <id>] --lang <en|es>
   nebula log-end --a2u '<scores-json>' \
     [--outcome <completed|partial|abandoned>] [--tip '<tip>'] \
     --lang <en|es>
   ```

`--tip` is written live by the agent. There is no tip catalog in the CLI,
database, locale catalogs, or this skill. Pass it only when `tips` says the
current prompt's `u2a` is worse than the same-session conversation baseline.
The stop hook enforces the same gate and discards a tip otherwise. Tips are
transient and never stored in SQLite.

Stops before `log-end`, such as waits for subagents, refresh metrics but keep
the turn open. If `log-end` never runs, the next prompt closes the previous turn
at its last stop. The stop hook closes completed turns and displays the compact
metrics block; do not print that block yourself.

Use `--session` only when the hook or caller supplied the identifier. Never
invent one for an unrelated open turn. Use 2–6 lowercase English domain
keywords, such as `rust,sqlite,release`. `--correction` accepts only `0` or `1`.

## Writing a live tip

1. Read `nebula tips`. Its first line says whether current `u2a` is worse than
   the recent same-session baseline. The table shows recent-window average,
   earlier average, delta, and last prompt for each sub-metric.
2. Choose one concrete change with the largest expected gain. Prefer high
   window values, non-improving deltas, and the current prompt's worst values.
3. Write one direct action in the session language, grounded in how the user
   writes. Maximum 90 characters. No examples, quotes, or explanation. A longer
   tip fails; Nebula never truncates it.

Do not pass a tip when the gate is off. Do not repeat the previous tip unless
the numbers still identify the same problem.

## Language

Pass `--lang en` or `--lang es` to match the conversation. Resolution order is
explicit option, current session language, `NEBULA_LANG`, then English. Each
command uses one complete embedded catalog and never mixes languages. Unknown
language codes or incomplete catalogs fail. Database keys and keywords remain
unchanged while visible metric labels are localized.

## Linguistic nebulosity

Nebulosity is lack of clarity, precision, or transparency that makes text hard
to understand. Higher is worse. Score both directions independently:

- `u2a`: user prompt as received;
- `a2u`: final answer as the user will read it.

Supply every key below as a number from 0 through 100:

| Key | Measures |
| --- | --- |
| `ambiguity` | Multiple plausible readings |
| `idea_branching` | Nested or parallel idea trees |
| `idea_dispersion` | Ideas without a common thread |
| `context_gap` | Missing context needed to act |
| `explanation_gap` | Missing explanations |
| `argument_gap` | Holes in reasoning |
| `lucidity_deficit` | Lack of lucidity |
| `structural_complexity` | Convoluted structure |
| `opacity` | Intent is hard to see |
| `certainty_deficit` | Lack of certainty |

Anchors: 0–20 crisp, 21–40 minor gaps, 41–60 needs assumptions, 61–80
likely misread, 81–100 cannot act. Judge each message in isolation. When a
message directly answers the other side's question, use `context_gap: 0`.
Hooks also store heuristic values and unmarked questions for bias analysis.

Example:

```json
{"ambiguity":30,"idea_branching":20,"idea_dispersion":10,"context_gap":25,"explanation_gap":15,"argument_gap":10,"lucidity_deficit":15,"structural_complexity":20,"opacity":15,"certainty_deficit":5}
```

## Reports and guidance

```sh
nebula tips [--share <0.3..1.0>] [--both] [--all] [--session <id>] [--lang L]
nebula report [u2a|a2u] [--days N] [--keyword K] [--repo SUBSTR] \
  [--recs <json-or-file>] [--lang L] [--format <md|html|json|csv|pdf>] [-o FILE]
```

`tips` defaults to `u2a` and the latest 30% of scored turns in the current
conversation. Values below 0.3 are clamped to 0.3; `--both` adds `a2u`, `--all`
uses all sessions, and `--session` selects a known session. The output is data
for the agent to interpret, not a catalog of generated advice.

Before producing a report with recommendations, run `nebula tips --both` and
write concrete actions derived from those numbers. Pass them inline or through
a local JSON file:

```json
{"you":["..."],"me":["..."],"both":["..."]}
```

Use `--recs '<json-or-file>'`. Without it, the report states that agent-written
recommendations were not supplied. A `u2a` or `a2u` report accepts only the
relevant recommendation group. SWOT remains rule-based and numeric. When the
user requests `/nebula report`, relay Markdown stdout verbatim; do not
summarize or retranslate it.

`--format` is preferred:

- `md`, `json`, and `csv` write stdout unless `-o/--output` is given;
- `html` and `pdf` create self-contained local files under
  `~/.nebula/reports` unless an output path is given;
- HTML has light/dark styling; JSON preserves the typed report model; CSV uses
  `section,direction,item,label,measure,value,text`; PDF needs no browser.

`--export <md|html|json|csv|pdf>` remains a compatibility alias with legacy
file behavior: without `--output`, it writes a timestamped file in the current
directory. Explicit paths and generated names never overwrite existing files.
Exports are local only. Never publish a report through cloud services or
connectors unless explicitly asked.

## Learning and historical import

- `nebula calibrate [--min-samples 20] [--lang L]` fits linear heuristic to
  model mappings per sub-metric and direction. Raw values stay unchanged.
- `nebula fit [--dry-run] [--min-samples 40] [--lang L]` reconstructs
  transcript text in memory, evaluates ridge models with five-fold
  cross-validation, adopts only models that clear the threshold and beat the
  existing rule, recomputes heuristic values, and resets calibration. Run after
  roughly 50 new turns.
- `nebula import [--sessions N] [--exclude ID ...] [--dump FILE] [--lang L]`
  discovers recent Claude and Codex transcripts and reconstructs numeric
  metrics. Existing sessions are skipped. `NEBULA_TRANSCRIPTS` replaces
  default discovery roots with an OS path list. Dumps use
  `nebula.safe-import-dump.v1` and contain safe metadata only.
- `nebula import-scores FILE [--lang L]` loads reviewed scores and metadata for
  imported turn IDs. Unknown turn IDs are skipped.

`fit` and `import` depend on locally available supported transcripts. Missing
transcripts reduce usable samples; they never relax the privacy invariant.

## Metric catalog

```sh
nebula metrics [--lang L] list
nebula metrics [--lang L] add KEY NAME CATEGORY UNIT <lower|higher> \
  [--parent KEY] [--weight W] [--aggregation NAME] [--directional]
```

Schema includes `sessions`, `turns`, `keywords`, `turn_keywords`,
`metric_definitions`, `metric_values`, learned heuristic weights, and
calibration. Reporting views include `v_turn_summary`, `v_nebulosity`,
`v_keyword_stats`, and `v_bias`.
