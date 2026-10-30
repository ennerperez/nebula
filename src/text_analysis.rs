use crate::{
    cli::Scope,
    i18n::Localizer,
    scoring::{self, KEYS},
};
use anyhow::{Result, bail};

pub(crate) fn analyze(text: &str, direction: Scope, localizer: &Localizer) -> Result<()> {
    let metrics = scoring::heuristic(text, direction == Scope::A2u);
    if metrics.is_empty() {
        bail!(localizer.text("analysis.empty"));
    }

    let nebulosity = KEYS
        .iter()
        .map(|key| metrics.get(*key).copied().unwrap_or_default())
        .sum::<f64>()
        / KEYS.len() as f64;
    let direction = match direction {
        Scope::U2a => "U2A",
        Scope::A2u => "A2U",
    };
    println!(
        "{}",
        localizer.format("analysis.header", &[("direction", direction.into())])
    );
    println!(
        "{}",
        localizer.format(
            "analysis.aggregate",
            &[("value", format!("{nebulosity:.1}"))]
        )
    );
    for key in KEYS {
        println!(
            "- {}: {:.1}",
            localizer.text(&format!("metric.{key}")),
            metrics.get(key).copied().unwrap_or_default()
        );
    }
    Ok(())
}
