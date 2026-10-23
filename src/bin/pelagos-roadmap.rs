//! Check or regenerate the evidence-driven human roadmap from docs/progress.json.
//! No Python, network access or runtime Jellyfin instance is required.
use std::{
    collections::HashSet,
    fmt::Write as _,
    fs,
    process::{ExitCode, exit},
};

use serde_json::Value;

fn string<'a>(value: &'a Value, field: &str) -> &'a str {
    value[field]
        .as_str()
        .unwrap_or_else(|| panic!("missing string field {field}"))
}

fn children<'a>(value: &'a Value, field: &str) -> &'a [Value] {
    value[field]
        .as_array()
        .unwrap_or_else(|| panic!("missing array field {field}"))
}

fn percentage(values: &[usize]) -> usize {
    if values.is_empty() {
        0
    } else {
        (values.iter().sum::<usize>() + values.len() / 2) / values.len()
    }
}

fn bar(value: usize) -> String {
    let filled = (value + 5) / 10;
    format!("{}{}", "█".repeat(filled), "░".repeat(10 - filled))
}

fn task_progress(task: &Value, ids: &mut HashSet<String>) -> Result<usize, String> {
    let id = string(task, "id");
    if !ids.insert(id.to_string()) {
        return Err(format!("duplicate task ID: {id}"));
    }
    let checks = children(task, "checks");
    if checks.is_empty() {
        return Err(format!("{id}: no acceptance checks"));
    }
    let mut passed = 0;
    for check in checks {
        let accepted = check["passed"]
            .as_bool()
            .ok_or_else(|| format!("{id}: check missing boolean passed"))?;
        if accepted {
            let evidence = check["evidence"]
                .as_str()
                .ok_or_else(|| format!("{id}: accepted check missing evidence"))?;
            if evidence.trim().is_empty() {
                return Err(format!("{id}: accepted check has empty evidence"));
            }
            passed += 1;
        }
    }
    let expected = (passed * 100 + checks.len() / 2) / checks.len();
    let actual = task["progress"]
        .as_u64()
        .ok_or_else(|| format!("{id}: task progress must be an unsigned integer"))?
        as usize;
    if expected != actual {
        return Err(format!(
            "{id}: claimed {actual}%, accepted checks yield {expected}%"
        ));
    }
    Ok(expected)
}

fn render(ledger: &Value) -> Result<String, String> {
    if ledger["schema"] != 1 || string(ledger, "project") != "pelagos" {
        return Err("unsupported ledger schema/project".into());
    }
    let milestones = children(ledger, "milestones");
    let mut ids = HashSet::new();
    let mut all = Vec::new();
    for milestone in milestones {
        for phase in children(milestone, "phases") {
            for task in children(phase, "tasks") {
                all.push(task_progress(task, &mut ids)?);
            }
        }
    }
    let overall = percentage(&all);
    let mut out = format!(
        "# 🗺️ Pelagos full-port roadmap\n\n\
         **MVP means complete Jellyfin Server port**, not a slim movie streamer. Scope: [Jellyfin v12.3 baseline](SOURCE_BASELINE.md) plus compatibility with unmodified Smart TV Jellyfin clients. Every milestone M0 through M6 is part of MVP. Full source of truth: [progress.json](progress.json).\n\n\
         **Overall acceptance progress:** \x60{} {}%\x60 across {} atomic tasks. This tracks accepted checks, not lines translated or claimed runtime readiness. Checked source-path milestones are not substitutes for passing CI or real devices.\n\n",
        bar(overall),
        overall,
        all.len(),
    );
    for milestone in milestones {
        let mut milestone_values = Vec::new();
        for phase in children(milestone, "phases") {
            for task in children(phase, "tasks") {
                milestone_values.push(task["progress"].as_u64().unwrap() as usize);
            }
        }
        let mp = percentage(&milestone_values);
        writeln!(
            out,
            "## {} {} — {} — \x60{} {}%\x60\n",
            string(milestone, "emoji"),
            string(milestone, "id"),
            string(milestone, "name"),
            bar(mp),
            mp
        )
        .unwrap();
        writeln!(out, "**Gate:** {}\n", string(milestone, "gate")).unwrap();
        for phase in children(milestone, "phases") {
            let values: Vec<usize> = children(phase, "tasks")
                .iter()
                .map(|t| t["progress"].as_u64().unwrap() as usize)
                .collect();
            let pp = percentage(&values);
            writeln!(
                out,
                "### {} {} — {} — \x60{} {}%\x60\n",
                string(phase, "emoji"),
                string(phase, "id"),
                string(phase, "name"),
                bar(pp),
                pp
            )
            .unwrap();
            for task in children(phase, "tasks") {
                let value = task["progress"].as_u64().unwrap() as usize;
                writeln!(
                    out,
                    "- {} **{} — {}** — \x60{} {}%\x60",
                    string(task, "emoji"),
                    string(task, "id"),
                    string(task, "name"),
                    bar(value),
                    value
                )
                .unwrap();
                for check in children(task, "checks") {
                    let accepted = check["passed"].as_bool().unwrap();
                    let evidence = if accepted {
                        format!(" — {}", string(check, "evidence"))
                    } else {
                        String::new()
                    };
                    writeln!(
                        out,
                        "  - [{}] {}{}",
                        if accepted { "x" } else { " " },
                        string(check, "title"),
                        evidence
                    )
                    .unwrap();
                }
            }
            out.push('\n');
        }
    }
    Ok(out)
}

fn run() -> Result<(), Box<dyn std::error::Error>> {
    let ledger: Value = serde_json::from_str(&fs::read_to_string("docs/progress.json")?)?;
    let actual = render(&ledger)?;
    match std::env::args().nth(1).as_deref() {
        Some("--write") => fs::write("docs/ROADMAP.md", actual)?,
        Some("--check") => {
            if actual != fs::read_to_string("docs/ROADMAP.md")? {
                return Err("docs/ROADMAP.md differs from the ledger. Run cargo run --bin pelagos-roadmap -- --write".into());
            }
        }
        None | Some("--print") => print!("{actual}"),
        Some(other) => return Err(format!("unknown argument: {other}").into()),
    }
    Ok(())
}

fn main() -> ExitCode {
    if let Err(err) = run() {
        eprintln!("pelagos-roadmap: {err}");
        exit(1);
    }
    ExitCode::SUCCESS
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn percent_and_bar() {
        assert_eq!(percentage(&[33, 67]), 50);
        assert_eq!(bar(50), "█████░░░░░");
    }

    #[test]
    fn accepts_current_ledger() {
        let doc: Value = serde_json::from_str(include_str!("../../docs/progress.json")).unwrap();
        let expected = render(&doc).unwrap();
        assert!(!expected.is_empty());
        assert!(expected.contains("MVP means complete Jellyfin Server port"));
    }
}
