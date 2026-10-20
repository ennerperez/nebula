use crate::{db, i18n::Lang};
use anyhow::{Context, Result};
use rusqlite::Connection;
use serde_json::{Map, Value};
use std::{fs, path::PathBuf};

pub(crate) struct App {
    home: PathBuf,
}

impl App {
    pub(crate) fn new() -> Result<Self> {
        Ok(Self {
            home: std::env::var_os("NEBULA_HOME")
                .map(PathBuf::from)
                .or_else(|| dirs::home_dir().map(|p| p.join(".nebula")))
                .context("no home")?,
        })
    }

    pub(crate) fn db(&self) -> Result<Connection> {
        fs::create_dir_all(&self.home)?;
        let connection = Connection::open(self.home.join("metrics.db"))?;
        db::initialize(&connection)?;
        Ok(connection)
    }

    pub(crate) fn reports_dir(&self) -> PathBuf {
        self.home.join("reports")
    }

    #[cfg(test)]
    pub(crate) fn for_test(home: PathBuf) -> Self {
        Self { home }
    }

    pub(crate) fn state_path(&self, session: &str) -> PathBuf {
        self.home
            .join("state")
            .join(format!("{}.json", session_key(session)))
    }

    pub(crate) fn language_path(&self, session: &str) -> PathBuf {
        self.home
            .join("state")
            .join(format!("{}.lang", session_key(session)))
    }

    pub(crate) fn load_state(&self, session: &str) -> Map<String, Value> {
        fs::read_to_string(self.state_path(session))
            .ok()
            .and_then(|contents| serde_json::from_str(&contents).ok())
            .unwrap_or_default()
    }

    pub(crate) fn save_state(&self, session: &str, state: &Map<String, Value>) -> Result<()> {
        fs::create_dir_all(self.home.join("state"))?;
        fs::write(self.state_path(session), serde_json::to_vec(state)?)?;
        Ok(())
    }

    pub(crate) fn session_language(&self, session: &str) -> Option<String> {
        fs::read_to_string(self.language_path(session))
            .ok()
            .map(|value| value.trim().to_owned())
            .filter(|value| !value.is_empty())
    }

    pub(crate) fn save_session_language(&self, session: &str, lang: Lang) -> Result<()> {
        fs::create_dir_all(self.home.join("state"))?;
        fs::write(self.language_path(session), lang.code())?;
        Ok(())
    }
}

fn session_key(session: &str) -> String {
    if session.is_empty() {
        return "~00".into();
    }
    let mut output = String::with_capacity(session.len());
    for byte in session.bytes() {
        if byte.is_ascii_alphanumeric() || matches!(byte, b'-' | b'_') {
            output.push(char::from(byte));
        } else {
            output.push_str(&format!("~{byte:02X}"));
        }
    }
    output
}

#[cfg(test)]
#[cfg_attr(coverage_nightly, coverage(off))]
mod tests {
    use super::*;
    use std::sync::atomic::{AtomicU64, Ordering};

    static NEXT_HOME: AtomicU64 = AtomicU64::new(0);

    fn test_app() -> App {
        let n = NEXT_HOME.fetch_add(1, Ordering::Relaxed);
        App {
            home: std::env::temp_dir().join(format!("nebula-test-{}-{n}", std::process::id())),
        }
    }

    #[test]
    fn app_persists_state_and_initializes_seeded_database() {
        let app = test_app();
        assert_eq!(
            app.state_path("session/../unsafe"),
            app.home.join("state/session~2F~2E~2E~2Funsafe.json")
        );
        assert_ne!(app.state_path("a/b"), app.state_path("ab"));
        assert_ne!(app.state_path("~2F"), app.state_path("/"));
        assert_eq!(app.state_path(""), app.home.join("state/~00.json"));

        let mut state = Map::new();
        state.insert("closed".into(), true.into());
        app.save_state("session-1", &state).unwrap();
        assert_eq!(app.load_state("session-1"), state);
        fs::write(app.state_path("session-1"), "not json").unwrap();
        assert!(app.load_state("session-1").is_empty());
        assert!(app.load_state("missing").is_empty());
        app.save_session_language("session-1", Lang::Es).unwrap();
        assert_eq!(app.session_language("session-1").as_deref(), Some("es"));
        assert_eq!(
            app.language_path("session/../unsafe"),
            app.home.join("state/session~2F~2E~2E~2Funsafe.lang")
        );

        let connection = app.db().unwrap();
        let count: i64 = connection
            .query_row("SELECT COUNT(*) FROM metric_definitions", [], |row| {
                row.get(0)
            })
            .unwrap();
        assert_eq!(count, 22);
        drop(connection);
        assert_eq!(
            app.db()
                .unwrap()
                .query_row("SELECT COUNT(*) FROM metric_definitions", [], |row| row
                    .get::<_, i64>(0))
                .unwrap(),
            22
        );
        fs::remove_dir_all(&app.home).unwrap();
    }
}
