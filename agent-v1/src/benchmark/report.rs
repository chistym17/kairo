use std::fs;
use std::path::Path;

use anyhow::{Context, Result};

use super::metrics::BenchmarkReport;

pub fn print_summary(report: &BenchmarkReport) {
    println!("\n[benchmark] ── summary ─────────────────────────────");
    println!(
        "[benchmark] pass rate: {}/{} ({:.0}%)",
        report.benchmark_passed,
        report.total_runs,
        report.pass_rate * 100.0
    );

    if let Some(avg) = report.avg_steps_on_success {
        println!("[benchmark] avg steps (passed runs): {avg:.1}");
    }

    if !report.failure_counts.is_empty() {
        println!("[benchmark] failures:");
        for (category, count) in &report.failure_counts {
            println!("  - {category}: {count}");
        }
    }

    println!("\n[benchmark] per-run results:");
    for trial in &report.trials {
        let status = if trial.benchmark_passed {
            "PASS"
        } else {
            "FAIL"
        };
        let category = trial
            .failure_category
            .map(|c| c.as_str())
            .unwrap_or("—");
        println!(
            "  [{status}] {} trial {} | steps={} | stop={:?} | category={}",
            trial.task_id, trial.trial, trial.total_steps, trial.stop_reason, category
        );
    }

    println!("\n[benchmark] analysis:");
    println!("  {}", report.analysis);
}

pub fn write_reports(output_dir: &Path, report: &BenchmarkReport) -> Result<()> {
    fs::create_dir_all(output_dir)?;

    let json_path = output_dir.join("report.json");
    let json = serde_json::to_string_pretty(report).context("failed to serialize report")?;
    fs::write(&json_path, json)?;

    let md_path = output_dir.join("report.md");
    fs::write(&md_path, render_markdown(report))?;

    println!("[benchmark] wrote {}", json_path.display());
    println!("[benchmark] wrote {}", md_path.display());

    Ok(())
}

fn render_markdown(report: &BenchmarkReport) -> String {
    let mut md = String::from("# Benchmark Report\n\n");
    md.push_str(&format!(
        "- **Pass rate:** {}/{} ({:.0}%)\n",
        report.benchmark_passed,
        report.total_runs,
        report.pass_rate * 100.0
    ));

    if let Some(avg) = report.avg_steps_on_success {
        md.push_str(&format!("- **Avg steps (passed):** {avg:.1}\n"));
    }

    md.push_str("\n## Failure breakdown\n\n");
    if report.failure_counts.is_empty() {
        md.push_str("No failures.\n");
    } else {
        md.push_str("| Category | Count |\n|----------|-------|\n");
        for (category, count) in &report.failure_counts {
            md.push_str(&format!("| {category} | {count} |\n"));
        }
    }

    md.push_str("\n## Per-run results\n\n");
    md.push_str("| Task | Trial | Result | Steps | Stop reason | Category |\n");
    md.push_str("|------|-------|--------|-------|-------------|----------|\n");
    for trial in &report.trials {
        let result = if trial.benchmark_passed { "PASS" } else { "FAIL" };
        let category = trial
            .failure_category
            .map(|c| c.as_str())
            .unwrap_or("—");
        md.push_str(&format!(
            "| {} | {} | {} | {} | {:?} | {} |\n",
            trial.task_id, trial.trial, result, trial.total_steps, trial.stop_reason, category
        ));
    }

    md.push_str("\n## Analysis\n\n");
    md.push_str(&report.analysis);
    md.push('\n');

    md
}
