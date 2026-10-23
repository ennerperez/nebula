use anyhow::{Context, Result};
use serde_json::{Map, Value, json};
use std::{
    fs::{self, OpenOptions},
    io::Write,
    path::{Path, PathBuf},
    sync::atomic::{AtomicU64, Ordering},
};

const CLAUDE_EVENTS: [(&str, &str); 3] = [
    ("UserPromptSubmit", "user-prompt-submit"),
    ("Stop", "stop"),
    ("Notification", "notification"),
];
const CODEX_EVENTS: [(&str, &str); 3] = [
    ("UserPromptSubmit", "user-prompt-submit"),
    ("Stop", "stop"),
    ("PermissionRequest", "notification"),
];
static NEXT_TEMPORARY: AtomicU64 = AtomicU64::new(0);

pub(crate) struct ConfigureResult {
    pub(crate) changed: bool,
    pub(crate) backup: Option<PathBuf>,
}

pub(crate) fn configure_claude(settings: &Path, binary: &Path) -> Result<ConfigureResult> {
    configure_hooks(settings, binary, "Claude", &CLAUDE_EVENTS)
}

pub(crate) fn configure_codex(hooks: &Path, binary: &Path) -> Result<ConfigureResult> {
    configure_hooks(hooks, binary, "Codex", &CODEX_EVENTS)
}

fn configure_hooks(
    settings: &Path,
    binary: &Path,
    host: &str,
    events: &[(&str, &str)],
) -> Result<ConfigureResult> {
    let binary = binary
        .to_str()
        .context("Nebula binary path must be valid UTF-8")?;
    let original = match fs::read(settings) {
        Ok(contents) => Some(contents),
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => None,
        Err(error) => {
            return Err(error).with_context(|| format!("cannot read {}", settings.display()));
        }
    };
    let mut root = match original.as_deref() {
        Some(contents) => serde_json::from_slice::<Value>(contents)
            .with_context(|| format!("{} contains invalid JSON", settings.display()))?,
        None => json!({}),
    };
    let object = root
        .as_object_mut()
        .with_context(|| format!("{} must contain a JSON object", settings.display()))?;
    let hooks = object
        .entry("hooks")
        .or_insert_with(|| Value::Object(Map::new()))
        .as_object_mut()
        .with_context(|| format!("{host} settings 'hooks' must be a JSON object"))?;

    let agent_home = settings
        .parent()
        .filter(|parent| !parent.as_os_str().is_empty())
        .unwrap_or_else(|| Path::new("."));
    let agent_home = agent_home
        .to_str()
        .with_context(|| format!("{host} settings directory must be valid UTF-8"))?;
    let quoted_agent_home = command_quote(agent_home);
    let quoted_binary = hook_binary_command(binary, host);
    let mut changed = false;
    for (event, nebula_event) in events {
        let command = format!(
            "{quoted_binary} --agent-home {quoted_agent_home} hook {nebula_event}"
        );
        changed |= merge_hook(hooks, event, nebula_event, &command, host)?;
    }
    if !changed {
        return Ok(ConfigureResult {
            changed: false,
            backup: None,
        });
    }

    let parent = settings
        .parent()
        .filter(|parent| !parent.as_os_str().is_empty())
        .unwrap_or_else(|| Path::new("."));
    fs::create_dir_all(parent).with_context(|| format!("cannot create {}", parent.display()))?;
    let backup = if let Some(contents) = original.as_ref() {
        let path = backup_path(settings);
        if path.exists() {
            None
        } else {
            write_backup(&path, contents, fs::metadata(settings)?.permissions())?;
            Some(path)
        }
    } else {
        None
    };

    let mut output = serde_json::to_vec_pretty(&root)?;
    output.push(b'\n');
    atomic_write(settings, &output, original.is_some())?;
    Ok(ConfigureResult {
        changed: true,
        backup,
    })
}

fn merge_hook(
    hooks: &mut Map<String, Value>,
    event: &str,
    nebula_event: &str,
    command: &str,
    host: &str,
) -> Result<bool> {
    let entries = hooks
        .entry(event)
        .or_insert_with(|| Value::Array(Vec::new()))
        .as_array_mut()
        .with_context(|| format!("{host} hook '{event}' must be an array"))?;
    let mut found = false;
    let mut changed = false;

    entries.retain_mut(|entry| {
        let Some(commands) = entry.get_mut("hooks").and_then(Value::as_array_mut) else {
            return true;
        };
        commands.retain_mut(|hook| {
            let Some(existing) = hook.get("command").and_then(Value::as_str) else {
                return true;
            };
            if !is_nebula_hook(existing, nebula_event) {
                return true;
            }
            if found {
                changed = true;
                return false;
            }
            found = true;
            if existing != command {
                hook["command"] = Value::String(command.to_owned());
                changed = true;
            }
            true
        });
        if commands.is_empty() {
            changed = true;
            false
        } else {
            true
        }
    });

    if !found {
        entries.push(json!({
            "hooks": [{
                "type": "command",
                "command": command
            }]
        }));
        changed = true;
    }
    Ok(changed)
}

fn is_nebula_hook(command: &str, event: &str) -> bool {
    let trimmed = command.trim();
    trimmed.ends_with(&format!("hook {event}"))
        && trimmed.rsplit_once(" hook ").is_some_and(|(program, _)| {
            let program = program
                .split(" --agent-home ")
                .next()
                .unwrap_or(program)
                .split_whitespace()
                .next_back()
                .unwrap_or(program);
            matches!(
                program
                    .rsplit(['/', '\\'])
                    .next()
                    .map(|name| name.trim_matches(['\'', '"'])),
                Some("nebula" | "nebula.exe" | "nebular.py")
            )
        })
}

fn command_quote(value: &str) -> String {
    if cfg!(windows) {
        format!("\"{}\"", value.replace('\\', "/"))
    } else {
        format!("'{}'", value.replace('\'', "'\\''"))
    }
}

fn hook_binary_command(binary: &str, host: &str) -> String {
    if host == "Codex"
        && cfg!(windows)
        && binary
            .chars()
            .all(|character| character.is_ascii_alphanumeric() || ":/\\._-".contains(character))
    {
        binary.replace('\\', "/")
    } else {
        command_quote(binary)
    }
}

fn backup_path(settings: &Path) -> PathBuf {
    settings.with_file_name(format!(
        "{}.bak-nebula",
        settings
            .file_name()
            .and_then(|name| name.to_str())
            .unwrap_or("settings.json")
    ))
}

fn write_backup(path: &Path, contents: &[u8], permissions: fs::Permissions) -> Result<()> {
    let mut file = OpenOptions::new()
        .write(true)
        .create_new(true)
        .open(path)
        .with_context(|| format!("cannot create backup {}", path.display()))?;
    let result = file
        .set_permissions(permissions)
        .and_then(|_| file.write_all(contents))
        .and_then(|_| file.sync_all());
    if let Err(error) = result {
        drop(file);
        let _ = fs::remove_file(path);
        return Err(error).with_context(|| format!("cannot write backup {}", path.display()));
    }
    Ok(())
}

fn atomic_write(path: &Path, contents: &[u8], existed: bool) -> Result<()> {
    let file_name = path
        .file_name()
        .and_then(|name| name.to_str())
        .context("settings path must end in a valid UTF-8 file name")?;
    let id = NEXT_TEMPORARY.fetch_add(1, Ordering::Relaxed);
    let temporary = path.with_file_name(format!(".{file_name}.nebula-{}-{id}", std::process::id()));
    let mut file = OpenOptions::new()
        .write(true)
        .create_new(true)
        .open(&temporary)
        .with_context(|| format!("cannot create {}", temporary.display()))?;
    let result = (|| -> std::io::Result<()> {
        if existed {
            let permissions = fs::metadata(path)?.permissions();
            file.set_permissions(permissions)?;
        }
        #[cfg(unix)]
        if !existed {
            use std::os::unix::fs::PermissionsExt;
            file.set_permissions(fs::Permissions::from_mode(0o600))?;
        }
        file.write_all(contents)?;
        file.sync_all()
    })();
    if let Err(error) = result {
        drop(file);
        let _ = fs::remove_file(&temporary);
        return Err(error).with_context(|| format!("cannot write {}", temporary.display()));
    }
    drop(file);
    #[cfg(windows)]
    if existed {
        fs::remove_file(path)?;
    }
    if let Err(error) = fs::rename(&temporary, path) {
        let _ = fs::remove_file(&temporary);
        return Err(error).with_context(|| format!("cannot replace {}", path.display()));
    }
    Ok(())
}

#[cfg(test)]
#[cfg_attr(coverage_nightly, coverage(off))]
mod tests {
    use super::*;
    use std::sync::atomic::{AtomicU64, Ordering};

    static NEXT_DIR: AtomicU64 = AtomicU64::new(0);

    fn test_dir() -> PathBuf {
        let id = NEXT_DIR.fetch_add(1, Ordering::Relaxed);
        std::env::temp_dir().join(format!("nebula-integrations-{}-{id}", std::process::id()))
    }

    #[test]
    fn configures_claude_without_removing_existing_settings() {
        let root = test_dir();
        fs::create_dir_all(&root).unwrap();
        let settings = root.join("settings.json");
        fs::write(
            &settings,
            r#"{"permissions":{"allow":["Bash(git status)"]},"hooks":{"Stop":[{"hooks":[{"type":"command","command":"notify-send done"}]}]}}"#,
        )
        .unwrap();

        let result = configure_claude(&settings, Path::new("/opt/Nebula Tools/nebula")).unwrap();
        assert!(result.changed);
        assert_eq!(result.backup, Some(root.join("settings.json.bak-nebula")));
        assert_eq!(
            fs::read_to_string(root.join("settings.json.bak-nebula")).unwrap(),
            r#"{"permissions":{"allow":["Bash(git status)"]},"hooks":{"Stop":[{"hooks":[{"type":"command","command":"notify-send done"}]}]}}"#
        );
        let value: Value = serde_json::from_slice(&fs::read(&settings).unwrap()).unwrap();
        assert_eq!(value["permissions"]["allow"][0], "Bash(git status)");
        assert_eq!(value["hooks"]["Stop"].as_array().unwrap().len(), 2);
        for (event, nebula_event) in CLAUDE_EVENTS {
            let expected = format!(
                "{} --agent-home {} hook {}",
                command_quote("/opt/Nebula Tools/nebula"),
                command_quote(&root.to_string_lossy()),
                nebula_event
            );
            assert!(
                value["hooks"][event]
                    .as_array()
                    .unwrap()
                    .iter()
                    .any(|entry| {
                        entry["hooks"]
                            .as_array()
                            .unwrap()
                            .iter()
                            .any(|hook| hook["command"].as_str() == Some(expected.as_str()))
                    })
            );
        }

        let second = configure_claude(&settings, Path::new("/opt/Nebula Tools/nebula")).unwrap();
        assert!(!second.changed);
        assert!(second.backup.is_none());
        fs::remove_dir_all(root).unwrap();
    }

    #[test]
    fn replaces_legacy_hooks_and_rejects_invalid_json() {
        let root = test_dir();
        fs::create_dir_all(&root).unwrap();
        let settings = root.join("settings.json");
        fs::write(
            &settings,
            r#"{"hooks":{"Stop":[{"hooks":[{"type":"command","command":"nebula hook stop"},{"type":"command","command":"python3 '/home/demo/.claude-alt/skills/nebular/nebular.py' hook stop"}]}]}}"#,
        )
        .unwrap();
        configure_claude(&settings, Path::new("/custom/bin/nebula")).unwrap();
        let text = fs::read_to_string(&settings).unwrap();
        assert_eq!(text.matches("hook stop").count(), 1);
        assert!(text.contains(&format!(
            "{} --agent-home {} hook stop",
            command_quote("/custom/bin/nebula"),
            command_quote(&root.to_string_lossy())
        )));

        let invalid = root.join("invalid.json");
        fs::write(&invalid, "not-json").unwrap();
        assert!(configure_claude(&invalid, Path::new("/bin/nebula")).is_err());
        assert_eq!(fs::read_to_string(invalid).unwrap(), "not-json");
        fs::remove_dir_all(root).unwrap();
    }

    #[test]
    fn quotes_custom_paths_and_recognizes_windows_commands() {
        assert_eq!(
            command_quote("/tmp/O'Brien/nebula"),
            if cfg!(windows) {
                "\"/tmp/O'Brien/nebula\""
            } else {
                "'/tmp/O'\\''Brien/nebula'"
            }
        );
        assert!(is_nebula_hook(
            r#""C:\Nebula Tools\nebula.exe" hook stop"#,
            "stop"
        ));
        assert!(is_nebula_hook(
            "python3 '/home/demo/.claude-alt/skills/nebular/nebular.py' hook stop",
            "stop"
        ));
        assert!(is_nebula_hook(
            r#""C:/Nebula Tools/nebula.exe" --agent-home "C:/Users/demo/.claude" hook stop"#,
            "stop"
        ));
        if cfg!(windows) {
            assert_eq!(
                command_quote(r"C:\Users\demo\Nebula Tools\nebula.exe"),
                "\"C:/Users/demo/Nebula Tools/nebula.exe\""
            );
            assert_eq!(
                hook_binary_command(r"C:\Users\demo\.local\bin\nebula.exe", "Codex"),
                "C:/Users/demo/.local/bin/nebula.exe"
            );
            assert_eq!(
                hook_binary_command(r"C:\Nebula Tools\nebula.exe", "Codex"),
                "\"C:/Nebula Tools/nebula.exe\""
            );
        }
    }

    #[test]
    fn configures_codex_hooks_and_preserves_existing_entries() {
        let root = test_dir();
        fs::create_dir_all(&root).unwrap();
        let hooks = root.join("hooks.json");
        fs::write(
            &hooks,
            r#"{"description":"keep","hooks":{"Stop":[{"hooks":[{"type":"command","command":"notify-send done"}]}]}}"#,
        )
        .unwrap();

        let result = configure_codex(&hooks, Path::new("/opt/nebula")).unwrap();
        assert!(result.changed);
        assert_eq!(result.backup, Some(root.join("hooks.json.bak-nebula")));
        let value: Value = serde_json::from_slice(&fs::read(&hooks).unwrap()).unwrap();
        assert_eq!(value["description"], "keep");
        assert_eq!(value["hooks"]["Stop"].as_array().unwrap().len(), 2);
        for (event, nebula_event) in CODEX_EVENTS {
            let expected = format!(
                "{} --agent-home {} hook {nebula_event}",
                hook_binary_command("/opt/nebula", "Codex"),
                command_quote(&root.to_string_lossy())
            );
            assert!(
                value["hooks"][event]
                    .as_array()
                    .unwrap()
                    .iter()
                    .any(|entry| entry["hooks"]
                        .as_array()
                        .unwrap()
                        .iter()
                        .any(|hook| hook["command"].as_str() == Some(expected.as_str())))
            );
        }

        let second = configure_codex(&hooks, Path::new("/opt/nebula")).unwrap();
        assert!(!second.changed);
        assert!(second.backup.is_none());
        fs::remove_dir_all(root).unwrap();
    }
}
