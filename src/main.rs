#![cfg_attr(coverage_nightly, feature(coverage_attribute))]

mod analytics;
mod app;
mod cli;
mod commands;
mod db;
mod export;
mod hooks;
mod i18n;
mod import;
mod learning;
mod scoring;

use crate::{
    app::App,
    cli::{Cli, Cmd},
    i18n::{Lang, Localizer},
};
use anyhow::Result;
use clap::Parser;
use rusqlite::{Connection, OptionalExtension};
use std::io::{self, Read};

fn main() -> Result<()> {
    let cli = Cli::parse();
    let app = App::new()?;
    let connection = app.db()?;
    match cli.cmd {
        Cmd::Hook { event, lang } => {
            let mut input = String::new();
            io::stdin().read_to_string(&mut input)?;
            let payload = match parse_hook_payload(&input) {
                Ok(payload) => payload,
                Err(error) => {
                    let localizer = resolve_language(&app, &connection, lang, None)?;
                    eprintln!(
                        "{}",
                        localizer.format("hook.error", &[("error", error.message(&localizer))])
                    );
                    return Ok(());
                }
            };
            let session = payload
                .get("session_id")
                .and_then(serde_json::Value::as_str)
                .unwrap_or("unknown");
            let localizer = resolve_language(&app, &connection, lang, Some(session))?;
            if let Err(error) = hooks::handle(&app, &connection, event, payload, &localizer) {
                eprintln!(
                    "{}",
                    localizer.format("hook.error", &[("error", format!("{error:#}"))])
                )
            }
        }
        Cmd::LogStart(args) => {
            let localizer =
                resolve_language(&app, &connection, args.lang, args.session.as_deref())?;
            commands::log_start(&connection, args, &localizer)?
        }
        Cmd::LogEnd(args) => {
            let localizer =
                resolve_language(&app, &connection, args.lang, args.session.as_deref())?;
            commands::log_end(&app, &connection, args, &localizer)?
        }
        Cmd::Report {
            scope,
            days,
            keyword,
            repo,
            format,
            export,
            output,
            recs,
            lang,
        } => {
            let localizer = resolve_language(&app, &connection, lang, None)?;
            analytics::report(
                &app,
                &connection,
                scope,
                days,
                keyword,
                repo,
                format,
                export,
                output,
                recs,
                &localizer,
            )?
        }
        Cmd::Tips {
            share,
            both,
            all_history,
            session,
            lang,
        } => {
            let localizer = resolve_language(&app, &connection, lang, session.as_deref())?;
            analytics::tips(
                &connection,
                share,
                both,
                all_history,
                session.as_deref(),
                &localizer,
            )?
        }
        Cmd::Estimate { keywords, lang } => {
            let localizer = resolve_language(&app, &connection, lang, None)?;
            analytics::estimate(&connection, &keywords, &localizer)?
        }
        Cmd::Metrics { cmd, lang } => {
            let localizer = resolve_language(&app, &connection, lang, None)?;
            commands::metrics(&connection, cmd, &localizer)?
        }
        Cmd::Calibrate { min_samples, lang } => {
            let localizer = resolve_language(&app, &connection, lang, None)?;
            learning::calibrate(&connection, min_samples, &localizer)?
        }
        Cmd::Fit {
            min_samples,
            dry_run,
            lang,
        } => {
            let localizer = resolve_language(&app, &connection, lang, None)?;
            learning::fit(&connection, min_samples, dry_run, &localizer)?
        }
        Cmd::Import {
            sessions,
            dump,
            exclude,
            lang,
        } => {
            let localizer = resolve_language(&app, &connection, lang, None)?;
            import::import_sessions(&connection, sessions, &exclude, dump.as_deref(), &localizer)?
        }
        Cmd::ImportScores { file, lang } => {
            let localizer = resolve_language(&app, &connection, lang, None)?;
            import::import_scores(&connection, &file, &localizer)?
        }
    };
    Ok(())
}

enum HookPayloadError {
    InvalidJson(String),
    NotObject,
}

impl HookPayloadError {
    fn message(&self, localizer: &Localizer) -> String {
        match self {
            Self::InvalidJson(error) => {
                localizer.format("hook.invalid_json", &[("error", error.clone())])
            }
            Self::NotObject => localizer.text("hook.invalid_shape"),
        }
    }
}

fn parse_hook_payload(input: &str) -> std::result::Result<serde_json::Value, HookPayloadError> {
    let payload: serde_json::Value = serde_json::from_str(input)
        .map_err(|error| HookPayloadError::InvalidJson(error.to_string()))?;
    if !payload.is_object() {
        return Err(HookPayloadError::NotObject);
    }
    Ok(payload)
}

fn resolve_language(
    app: &App,
    connection: &Connection,
    explicit: Option<Lang>,
    requested_session: Option<&str>,
) -> Result<Localizer> {
    let session = match requested_session {
        Some(session) => Some(session.to_owned()),
        None => connection
            .query_row(
                "SELECT session_id FROM turns WHERE ended_at IS NULL ORDER BY started_at DESC LIMIT 1",
                [],
                |row| row.get(0),
            )
            .optional()?,
    };
    let stored = session
        .as_deref()
        .and_then(|session| app.session_language(session));
    let environment = std::env::var("NEBULAR_LANG").ok();
    let localizer = Localizer::resolve(explicit, stored.as_deref(), environment.as_deref())?;
    if explicit.is_some()
        && let Some(session) = session
    {
        app.save_session_language(&session, localizer.lang())?;
    }
    Ok(localizer)
}

#[cfg(test)]
mod tests {
    use super::parse_hook_payload;

    #[test]
    fn hook_payload_rejects_invalid_json_and_non_objects() {
        assert!(parse_hook_payload("not-json").is_err());
        assert!(parse_hook_payload("[]").is_err());
        assert!(parse_hook_payload(r#"{"session_id":"s"}"#).is_ok());
    }
}
