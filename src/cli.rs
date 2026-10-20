use crate::i18n::Lang;
use clap::{Args, Parser, Subcommand, ValueEnum};
use std::path::PathBuf;

#[derive(Parser)]
#[command(name = "nebula", version, about)]
pub(crate) struct Cli {
    #[command(subcommand)]
    pub(crate) cmd: Cmd,
}

#[derive(Subcommand)]
pub(crate) enum Cmd {
    Configure {
        #[command(subcommand)]
        cmd: ConfigureCmd,
    },
    Data {
        #[command(subcommand)]
        cmd: DataCmd,
    },
    Hook {
        #[arg(value_enum)]
        event: Event,
        #[arg(long, value_enum)]
        lang: Option<Lang>,
    },
    LogStart(Start),
    LogEnd(End),
    Report {
        #[arg(value_enum)]
        scope: Option<Scope>,
        #[arg(long, value_parser = clap::value_parser!(i64).range(1..))]
        days: Option<i64>,
        #[arg(long)]
        keyword: Option<String>,
        #[arg(long)]
        repo: Option<String>,
        #[arg(
            long,
            value_enum,
            help = "Render as md, html, json, csv, or pdf; md/json/csv use stdout by default"
        )]
        format: Option<ExportFormat>,
        #[arg(
            long,
            value_enum,
            help = "Export report to a local md, html, json, csv, or pdf file"
        )]
        export: Option<ExportFormat>,
        #[arg(
            long,
            short = 'o',
            value_name = "PATH",
            help = "Use an exact destination path; existing files are not overwritten"
        )]
        output: Option<PathBuf>,
        #[arg(long, value_name = "JSON_OR_FILE")]
        recs: Option<String>,
        #[arg(long, value_enum)]
        lang: Option<Lang>,
    },
    Tips {
        #[arg(long, default_value_t = 0.3, value_parser = finite_f64)]
        share: f64,
        #[arg(long)]
        both: bool,
        #[arg(long = "all")]
        all_history: bool,
        #[arg(long)]
        session: Option<String>,
        #[arg(long, value_enum)]
        lang: Option<Lang>,
    },
    Estimate {
        #[arg(long)]
        keywords: String,
        #[arg(long, value_enum)]
        lang: Option<Lang>,
    },
    Metrics {
        #[command(subcommand)]
        cmd: MetricCmd,
        #[arg(long, value_enum)]
        lang: Option<Lang>,
    },
    Calibrate {
        #[arg(
            long,
            default_value_t = 20,
            value_parser = positive_usize
        )]
        min_samples: usize,
        #[arg(long, value_enum)]
        lang: Option<Lang>,
    },
    Fit {
        #[arg(
            long,
            default_value_t = 40,
            value_parser = positive_usize
        )]
        min_samples: usize,
        #[arg(long)]
        dry_run: bool,
        #[arg(long, value_enum)]
        lang: Option<Lang>,
    },
    Import {
        #[arg(
            long,
            default_value_t = 10,
            value_parser = positive_usize
        )]
        sessions: usize,
        #[arg(long)]
        dump: Option<PathBuf>,
        #[arg(long, num_args = 0..)]
        exclude: Vec<String>,
        #[arg(long, value_enum)]
        host: Option<TranscriptHost>,
        #[arg(long, value_enum)]
        lang: Option<Lang>,
    },
    ImportScores {
        file: PathBuf,
        #[arg(long, value_enum)]
        lang: Option<Lang>,
    },
}

#[derive(Subcommand)]
pub(crate) enum ConfigureCmd {
    Claude {
        #[arg(long, value_name = "PATH")]
        settings: PathBuf,
        #[arg(long, value_name = "PATH")]
        binary: PathBuf,
    },
}

#[derive(Subcommand)]
pub(crate) enum DataCmd {
    Clear {
        #[arg(long, help = "Confirm permanent deletion of all Nebula-managed data")]
        yes: bool,
        #[arg(long, value_enum)]
        lang: Option<Lang>,
    },
}

#[derive(Clone, ValueEnum)]
pub(crate) enum Event {
    UserPromptSubmit,
    Stop,
    Notification,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq, ValueEnum)]
pub(crate) enum TranscriptHost {
    Claude,
    Codex,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq, ValueEnum)]
pub(crate) enum Scope {
    U2a,
    A2u,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq, ValueEnum)]
pub(crate) enum ExportFormat {
    Md,
    Html,
    Json,
    Csv,
    Pdf,
}

impl ExportFormat {
    pub(crate) fn extension(self) -> &'static str {
        match self {
            Self::Md => "md",
            Self::Html => "html",
            Self::Json => "json",
            Self::Csv => "csv",
            Self::Pdf => "pdf",
        }
    }
}

#[derive(Clone, Copy, Debug, Eq, PartialEq, ValueEnum)]
pub(crate) enum Outcome {
    Completed,
    Partial,
    Abandoned,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq, ValueEnum)]
pub(crate) enum Better {
    Lower,
    Higher,
}

impl Better {
    pub(crate) fn as_str(self) -> &'static str {
        match self {
            Self::Lower => "lower",
            Self::Higher => "higher",
        }
    }
}

impl Outcome {
    pub(crate) fn as_str(self) -> &'static str {
        match self {
            Self::Completed => "completed",
            Self::Partial => "partial",
            Self::Abandoned => "abandoned",
        }
    }
}

#[derive(Args)]
pub(crate) struct Start {
    #[arg(long, value_parser = positive_f64)]
    pub(crate) estimate: f64,
    #[arg(long)]
    pub(crate) keywords: String,
    #[arg(long = "type")]
    pub(crate) kind: Option<String>,
    #[arg(long, value_parser = clap::value_parser!(u64))]
    pub(crate) clarifying: Option<u64>,
    #[arg(long, value_parser = clap::value_parser!(u8).range(0..=1))]
    pub(crate) correction: Option<u8>,
    #[arg(long)]
    pub(crate) u2a: String,
    #[arg(long)]
    pub(crate) session: Option<String>,
    #[arg(long, value_enum)]
    pub(crate) lang: Option<Lang>,
}

#[derive(Args)]
pub(crate) struct End {
    #[arg(long)]
    pub(crate) a2u: String,
    #[arg(long)]
    pub(crate) outcome: Option<Outcome>,
    #[arg(long)]
    pub(crate) tip: Option<String>,
    #[arg(long)]
    pub(crate) session: Option<String>,
    #[arg(long, value_enum)]
    pub(crate) lang: Option<Lang>,
}

#[derive(Subcommand)]
pub(crate) enum MetricCmd {
    List,
    Add {
        key: String,
        name: String,
        category: String,
        unit: String,
        #[arg(value_enum)]
        better: Better,
        #[arg(long)]
        parent: Option<String>,
        #[arg(long, default_value_t = 1., value_parser = non_negative_f64)]
        weight: f64,
        #[arg(long, default_value = "sum")]
        aggregation: String,
        #[arg(long)]
        directional: bool,
    },
}

fn positive_f64(value: &str) -> Result<f64, String> {
    let parsed = value
        .parse::<f64>()
        .map_err(|_| "expected a number".to_owned())?;
    if parsed.is_finite() && parsed > 0.0 {
        Ok(parsed)
    } else {
        Err("expected a finite number greater than zero".into())
    }
}

fn positive_usize(value: &str) -> Result<usize, String> {
    let parsed = value
        .parse::<usize>()
        .map_err(|_| "expected a positive integer".to_owned())?;
    if parsed > 0 {
        Ok(parsed)
    } else {
        Err("expected an integer greater than zero".into())
    }
}

fn non_negative_f64(value: &str) -> Result<f64, String> {
    let parsed = value
        .parse::<f64>()
        .map_err(|_| "expected a number".to_owned())?;
    if parsed.is_finite() && parsed >= 0.0 {
        Ok(parsed)
    } else {
        Err("expected a finite non-negative number".into())
    }
}

fn finite_f64(value: &str) -> Result<f64, String> {
    let parsed = value
        .parse::<f64>()
        .map_err(|_| "expected a number".to_owned())?;
    if parsed.is_finite() {
        Ok(parsed)
    } else {
        Err("expected a finite number".into())
    }
}

#[cfg(test)]
#[cfg_attr(coverage_nightly, coverage(off))]
mod tests {
    use super::*;

    #[test]
    fn cli_parses_nested_and_value_enum_commands() {
        let configure = Cli::try_parse_from([
            "nebula",
            "configure",
            "claude",
            "--settings",
            "/tmp/claude/settings.json",
            "--binary",
            "/opt/nebula/bin/nebula",
        ])
        .unwrap();
        assert!(matches!(
            configure.cmd,
            Cmd::Configure {
                cmd: ConfigureCmd::Claude { settings, binary }
            } if settings == PathBuf::from("/tmp/claude/settings.json")
                && binary == PathBuf::from("/opt/nebula/bin/nebula")
        ));
        let data =
            Cli::try_parse_from(["nebula", "data", "clear", "--yes", "--lang", "es"]).unwrap();
        assert!(matches!(
            data.cmd,
            Cmd::Data {
                cmd: DataCmd::Clear {
                    yes: true,
                    lang: Some(Lang::Es)
                }
            }
        ));
        let hook =
            Cli::try_parse_from(["nebula", "hook", "user-prompt-submit", "--lang", "es"]).unwrap();
        assert!(matches!(
            hook.cmd,
            Cmd::Hook {
                event: Event::UserPromptSubmit,
                lang: Some(Lang::Es)
            }
        ));
        let metric = Cli::try_parse_from(["nebula", "metrics", "list"]).unwrap();
        assert!(matches!(
            metric.cmd,
            Cmd::Metrics {
                cmd: MetricCmd::List,
                ..
            }
        ));
        let report = Cli::try_parse_from([
            "nebula",
            "report",
            "u2a",
            "--days",
            "7",
            "--keyword",
            "rust",
            "--repo",
            "nebula",
            "--export",
            "json",
            "--output",
            "report.json",
        ])
        .unwrap();
        assert!(matches!(
            report.cmd,
            Cmd::Report {
                scope: Some(Scope::U2a),
                days: Some(7),
                export: Some(ExportFormat::Json),
                output: Some(output),
                ..
            } if output == PathBuf::from("report.json")
        ));
        assert!(Cli::try_parse_from(["nebula", "hook", "invalid"]).is_err());
    }

    #[test]
    fn cli_rejects_invalid_correction_outcome_scope_and_language() {
        assert!(
            Cli::try_parse_from([
                "nebula",
                "log-start",
                "--estimate",
                "1",
                "--keywords",
                "x",
                "--u2a",
                "{}",
                "--correction",
                "2"
            ])
            .is_err()
        );
        assert!(
            Cli::try_parse_from(["nebula", "log-end", "--a2u", "{}", "--outcome", "failed"])
                .is_err()
        );
        assert!(Cli::try_parse_from(["nebula", "report", "both"]).is_err());
        assert!(Cli::try_parse_from(["nebula", "report", "--days", "0"]).is_err());
        assert!(Cli::try_parse_from(["nebula", "tips", "--lang", "fr"]).is_err());
        assert!(Cli::try_parse_from(["nebula", "tips", "--share", "NaN"]).is_err());
        assert!(Cli::try_parse_from(["nebula", "report", "--export", "xml"]).is_err());
        assert!(Cli::try_parse_from(["nebula", "report", "--output", "report.md"]).is_ok());
        for estimate in ["0", "-1", "NaN", "inf"] {
            assert!(
                Cli::try_parse_from([
                    "nebula",
                    "log-start",
                    "--estimate",
                    estimate,
                    "--keywords",
                    "x",
                    "--u2a",
                    "{}"
                ])
                .is_err()
            );
        }
        assert!(
            Cli::try_parse_from([
                "nebula",
                "log-start",
                "--estimate",
                "1",
                "--keywords",
                "x",
                "--u2a",
                "{}",
                "--clarifying",
                "-1"
            ])
            .is_err()
        );
        assert!(Cli::try_parse_from(["nebula", "import", "--sessions", "0"]).is_err());
        assert!(Cli::try_parse_from(["nebula", "fit", "--min-samples", "0"]).is_err());
        assert!(Cli::try_parse_from(["nebula", "calibrate", "--min-samples", "0"]).is_err());
        assert!(
            Cli::try_parse_from([
                "nebula", "metrics", "add", "x", "X", "cat", "unit", "lower", "--weight", "NaN"
            ])
            .is_err()
        );
        assert!(
            Cli::try_parse_from([
                "nebula", "metrics", "add", "x", "X", "cat", "unit", "sideways"
            ])
            .is_err()
        );
    }

    #[test]
    fn tips_accepts_conversation_window_options() {
        let cli = Cli::try_parse_from([
            "nebula",
            "tips",
            "--share",
            "0.8",
            "--both",
            "--all",
            "--session",
            "s1",
        ])
        .unwrap();
        assert!(matches!(
            cli.cmd,
            Cmd::Tips {
                share,
                both: true,
                all_history: true,
                session: Some(session),
                ..
            } if share == 0.8 && session == "s1"
        ));
    }

    #[test]
    fn report_accepts_preferred_formats_outputs_and_recommendations() {
        let cli = Cli::try_parse_from([
            "nebula",
            "report",
            "a2u",
            "--format",
            "html",
            "-o",
            "report.html",
            "--recs",
            r#"{"me":["Be direct"],"both":[]}"#,
        ])
        .unwrap();
        assert!(matches!(
            cli.cmd,
            Cmd::Report {
                scope: Some(Scope::A2u),
                format: Some(ExportFormat::Html),
                output: Some(output),
                recs: Some(recs),
                ..
            } if output == PathBuf::from("report.html") && recs.contains("Be direct")
        ));
    }

    #[test]
    fn import_accepts_multiple_excluded_sessions() {
        let cli = Cli::try_parse_from([
            "nebula",
            "import",
            "--exclude",
            "one",
            "two",
            "--host",
            "claude",
            "--lang",
            "es",
        ])
        .unwrap();
        assert!(matches!(
            cli.cmd,
            Cmd::Import {
                exclude,
                host: Some(TranscriptHost::Claude),
                lang: Some(Lang::Es),
                ..
            } if exclude == ["one", "two"]
        ));
    }
}
