use anyhow::{Result, bail};
use clap::ValueEnum;
use serde_json::Value;
use std::collections::BTreeSet;

const EN: &str = include_str!("../locales/en.json");
const ES: &str = include_str!("../locales/es.json");

#[derive(Clone, Copy, Debug, Eq, PartialEq, ValueEnum)]
pub(crate) enum Lang {
    En,
    Es,
}

impl Lang {
    pub(crate) fn code(self) -> &'static str {
        match self {
            Self::En => "en",
            Self::Es => "es",
        }
    }

    pub(crate) fn parse(value: &str) -> Result<Self> {
        match value.trim().to_ascii_lowercase().as_str() {
            "en" => Ok(Self::En),
            "es" => Ok(Self::Es),
            unknown => bail!(
                "unknown language '{unknown}'; available: {}",
                Self::available().join(", ")
            ),
        }
    }

    pub(crate) fn available() -> [&'static str; 2] {
        ["en", "es"]
    }
}

#[derive(Clone, Debug)]
pub(crate) struct Localizer {
    lang: Lang,
    catalog: Value,
}

impl Localizer {
    pub(crate) fn load(lang: Lang) -> Result<Self> {
        let base: Value = serde_json::from_str(EN)?;
        let catalog: Value = serde_json::from_str(match lang {
            Lang::En => EN,
            Lang::Es => ES,
        })?;
        let base_keys = flat_keys(&base);
        let catalog_keys = flat_keys(&catalog);
        if base_keys != catalog_keys {
            let missing = base_keys
                .difference(&catalog_keys)
                .cloned()
                .collect::<Vec<_>>()
                .join(", ");
            bail!("locale '{}' is missing keys: {missing}", lang.code());
        }
        Ok(Self { lang, catalog })
    }

    pub(crate) fn resolve(
        explicit: Option<Lang>,
        session: Option<&str>,
        environment: Option<&str>,
    ) -> Result<Self> {
        let lang = if let Some(lang) = explicit {
            lang
        } else if let Some(session) = session.filter(|value| !value.trim().is_empty()) {
            Lang::parse(session)?
        } else if let Some(environment) = environment.filter(|value| !value.trim().is_empty()) {
            Lang::parse(environment)?
        } else {
            Lang::En
        };
        Self::load(lang)
    }

    pub(crate) fn lang(&self) -> Lang {
        self.lang
    }

    pub(crate) fn text(&self, path: &str) -> String {
        self.format(path, &[])
    }

    pub(crate) fn format(&self, path: &str, values: &[(&str, String)]) -> String {
        let mut node = &self.catalog;
        for part in path.split('.') {
            node = node
                .get(part)
                .unwrap_or_else(|| panic!("missing locale key: {path}"));
        }
        let mut output = node
            .as_str()
            .unwrap_or_else(|| panic!("locale key is not text: {path}"))
            .to_owned();
        for (key, value) in values {
            output = output.replace(&format!("{{{key}}}"), value);
        }
        output
    }
}

fn flat_keys(value: &Value) -> BTreeSet<String> {
    fn visit(value: &Value, prefix: &str, output: &mut BTreeSet<String>) {
        match value {
            Value::Object(entries) => {
                for (key, value) in entries {
                    let path = if prefix.is_empty() {
                        key.clone()
                    } else {
                        format!("{prefix}.{key}")
                    };
                    visit(value, &path, output);
                }
            }
            _ => {
                output.insert(prefix.to_owned());
            }
        }
    }

    let mut output = BTreeSet::new();
    visit(value, "", &mut output);
    output
}

#[cfg(test)]
#[cfg_attr(coverage_nightly, coverage(off))]
mod tests {
    use super::*;

    #[test]
    fn catalogs_are_complete_and_never_mix_languages() {
        let english = Localizer::load(Lang::En).unwrap();
        let spanish = Localizer::load(Lang::Es).unwrap();
        assert_eq!(english.text("report.no_data"), "no data");
        assert_eq!(spanish.text("report.no_data"), "sin datos");
    }

    #[test]
    fn resolution_uses_explicit_session_environment_default_precedence() {
        assert_eq!(
            Localizer::resolve(Some(Lang::Es), Some("en"), Some("en"))
                .unwrap()
                .lang(),
            Lang::Es
        );
        assert_eq!(
            Localizer::resolve(None, Some("es"), Some("en"))
                .unwrap()
                .lang(),
            Lang::Es
        );
        assert_eq!(
            Localizer::resolve(None, None, Some("es")).unwrap().lang(),
            Lang::Es
        );
        assert_eq!(
            Localizer::resolve(None, None, None).unwrap().lang(),
            Lang::En
        );
        assert!(Localizer::resolve(None, Some("fr"), None).is_err());
    }

    #[test]
    fn formatter_replaces_named_values() {
        let catalog = Localizer::load(Lang::En).unwrap();
        assert_eq!(
            catalog.format("admin.metric_added", &[("key", "quality".into())]),
            "metric quality added"
        );
    }
}
