use crate::{db, i18n::Lang};
use anyhow::{Context, Result, bail};
use rusqlite::Connection;
use serde_json::{Map, Value};
use std::{fs, path::PathBuf};

pub(crate) struct App {
    home: PathBuf,
}

#[derive(Debug)]
pub(crate) struct ClearResult {
    pub(crate) removed_paths: usize,
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

    pub(crate) fn home(&self) -> &std::path::Path {
        &self.home
    }

    pub(crate) fn clear_data(&self) -> Result<ClearResult> {
        let metadata = match fs::metadata(&self.home) {
            Ok(metadata) => metadata,
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => {
                return Ok(ClearResult { removed_paths: 0 });
            }
            Err(error) => return Err(error).context("cannot inspect Nebula data directory"),
        };
        if !metadata.is_dir() {
            bail!(
                "Nebula data path is not a directory: {}",
                self.home.display()
            );
        }
        let canonical =
            fs::canonicalize(&self.home).context("cannot resolve Nebula data directory")?;
        if canonical.parent().is_none() {
            bail!(
                "refusing to clear a filesystem root: {}",
                canonical.display()
            );
        }

        let mut removed_paths = 0;
        for name in [
            "metrics.db",
            "metrics.db-wal",
            "metrics.db-shm",
            "metrics.db-journal",
            "state",
            "reports",
        ] {
            let path = self.home.join(name);
            let metadata = match fs::symlink_metadata(&path) {
                Ok(metadata) => metadata,
                Err(error) if error.kind() == std::io::ErrorKind::NotFound => continue,
                Err(error) => {
                    return Err(error)
                        .with_context(|| format!("cannot inspect {}", path.display()));
                }
            };
            if metadata.file_type().is_dir() && !metadata.file_type().is_symlink() {
                fs::remove_dir_all(&path)
                    .with_context(|| format!("cannot remove {}", path.display()))?;
            } else {
                fs::remove_file(&path)
                    .with_context(|| format!("cannot remove {}", path.display()))?;
            }
            removed_paths += 1;
        }
        Ok(ClearResult { removed_paths })
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

    #[test]
    fn clear_data_removes_only_managed_paths_and_is_idempotent() {
        let app = test_app();
        let connection = app.db().unwrap();
        drop(connection);
        app.save_state("session-1", &Map::new()).unwrap();
        fs::create_dir_all(app.reports_dir()).unwrap();
        fs::write(app.reports_dir().join("report.html"), "report").unwrap();
        for sidecar in ["metrics.db-wal", "metrics.db-shm", "metrics.db-journal"] {
            fs::write(app.home.join(sidecar), sidecar).unwrap();
        }
        fs::write(app.home.join("keep.txt"), "keep").unwrap();

        let result = app.clear_data().unwrap();
        assert_eq!(result.removed_paths, 6);
        assert!(app.home.exists());
        assert_eq!(
            fs::read_to_string(app.home.join("keep.txt")).unwrap(),
            "keep"
        );
        for name in [
            "metrics.db",
            "metrics.db-wal",
            "metrics.db-shm",
            "metrics.db-journal",
            "state",
            "reports",
        ] {
            assert!(!app.home.join(name).exists());
        }
        assert_eq!(app.clear_data().unwrap().removed_paths, 0);
        fs::remove_dir_all(&app.home).unwrap();
    }

    #[test]
    fn clear_data_does_not_create_a_missing_home() {
        let app = test_app();
        assert!(!app.home.exists());
        assert_eq!(app.clear_data().unwrap().removed_paths, 0);
        assert!(!app.home.exists());
    }

    #[cfg(unix)]
    #[test]
    fn clear_data_unlinks_managed_symlinks_without_following_them() {
        use std::os::unix::fs::symlink;

        let app = test_app();
        let external = app.home.with_extension("external");
        fs::create_dir_all(&app.home).unwrap();
        fs::create_dir_all(&external).unwrap();
        fs::write(external.join("keep.txt"), "keep").unwrap();
        symlink(&external, app.home.join("state")).unwrap();

        assert_eq!(app.clear_data().unwrap().removed_paths, 1);
        assert_eq!(
            fs::read_to_string(external.join("keep.txt")).unwrap(),
            "keep"
        );
        fs::remove_dir_all(&app.home).unwrap();
        fs::remove_dir_all(external).unwrap();
    }

    #[cfg(unix)]
    #[test]
    fn clear_data_rejects_filesystem_root() {
        let app = App::for_test(PathBuf::from("/"));
        assert!(
            app.clear_data()
                .unwrap_err()
                .to_string()
                .contains("filesystem root")
        );
    }
}
