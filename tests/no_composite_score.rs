//! V2 removed the composite resilience score and its verdict tiers: the
//! scorecards report each metric on its own. Guards against their return.

const FILES: [(&str, &str); 4] = [
    ("web/app.js", include_str!("../web/app.js")),
    ("web/index.html", include_str!("../web/index.html")),
    ("web/styles.css", include_str!("../web/styles.css")),
    ("src/ui/desktop/panels/scorecard.rs", include_str!("../src/ui/desktop/panels/scorecard.rs")),
];

/// Matched case-insensitively; `verdict` covers every tier, banner and label.
const REMOVED: [&str; 5] = [
    "computecompositescore",
    "scorecard_weights",
    "verdict",
    "composite-formula",
    "composite_score",
];

#[test]
fn no_composite_score_or_verdict_tiers() {
    let found: Vec<String> = FILES
        .iter()
        .flat_map(|(path, text)| {
            let text = text.to_lowercase();
            REMOVED
                .iter()
                .filter(move |id| text.contains(*id))
                .map(move |id| format!("{path}: {id}"))
        })
        .collect();
    assert!(found.is_empty(), "composite score or verdict tiers are back:\n{}", found.join("\n"));
}
