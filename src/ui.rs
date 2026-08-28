use colored::*;
use std::fs::OpenOptions;
use std::io::{self, Write};
use std::path::{Path, PathBuf};

pub struct PackageSummary {
    pub name: String,
    pub version: String,
    pub source: String,
    pub size: u64,
    pub duration: std::time::Duration,
    pub checksum: String,
    pub status: String,
}

pub fn info(msg: &str) { println!("{} {}", "[i]".bright_cyan(), msg); }
pub fn success(msg: &str) { println!("{} {}", "[✓]".bright_green(), msg.bright_green()); }
pub fn warning(msg: &str) { println!("{} {}", "[!]".bright_yellow(), msg.bright_yellow()); }
pub fn error(msg: &str) { eprintln!("{} {}", "[✗]".bright_red(), msg.bright_red()); }
pub fn start_operation(msg: &str) { info(msg); }

pub fn dependency_report(required: &[(&str, bool)], optional: &[&str]) {
    let count = required.len() + optional.len();
    println!(
        "{} {}{}{}",
        "[!]".bright_yellow(),
        "Required dependencies found (".bright_yellow(),
        count,
        "):".bright_yellow()
    );
    for (dep, installed) in required.iter() {
        let status = if *installed {
            "installed".bright_green()
        } else {
            "missing".bright_red()
        };
        println!("    ‣ {} ({})", dep, status);
    }
    for dep in optional.iter() {
        println!("    ‣ {} ({})", dep, "optional".bright_yellow());
    }
}

pub fn conflict_report(conflicts: &[&str]) {
    println!(
        "{} {}{}{}",
        "[!]".bright_yellow(),
        "Conflicting packages (".bright_yellow(),
        conflicts.len(),
        "):".bright_yellow()
    );
    for dep in conflicts.iter() {
        println!("    ‣ {}", dep);
    }
}

pub fn reverse_dependency_report(dependents: &[String]) {
    println!(
        "{} {}{}{}",
        "[!]".bright_yellow(),
        "Required by this dependencies (".bright_yellow(),
        dependents.len(),
        "):".bright_yellow()
    );
    for dep in dependents.iter() {
        println!("    ‣ {}", dep);
    }
}

pub fn confirm(prompt: &str, default: bool) -> bool {
    let def = if default { "y" } else { "n" };
    print!("{} {} {}", "[?]".bright_yellow(), prompt.bright_yellow(), format!("[y/n] (default: {})\n    ‣ ", def).bright_yellow());
    io::stdout().flush().ok();
    let mut input = String::new();
    if io::stdin().read_line(&mut input).is_ok() {
        let v = input.trim().to_lowercase();
        if v.is_empty() {
            return default;
        }
        return v == "y" || v == "yes";
    }
    default
}

pub fn sum_success(msg: &str) { println!("{}", msg.bright_green()); }
pub fn sum_error(msg: &str) { println!("{}", msg.bright_red()); }

fn log_file_path(log_dir: &Path) -> PathBuf {
    let today = chrono::Local::now().format("%d-%m-%Y").to_string();
    log_dir.join(format!("{}.log", today))
}

fn render_operation_summary(entries: &[PackageSummary]) -> String {
    let now = chrono::Local::now();
    let mut out = String::new();
    out.push_str(&format!("‣ Date   : {}\n", now.format("%d-%m-%Y")));
    out.push_str(&format!("‣ Hour   : {}\n", now.format("%H:%M:%S %z")));
    out.push_str("Operation summary detailed log:\n");
    for entry in entries.iter() {
        out.push('\n');
        out.push_str(&format!("          ::: [ {} ({}) ] :::\n", entry.name, entry.version));
        out.push_str(&format!("        • Source      : {}\n", entry.source));
        out.push_str(&format!("        • Size        : {} bytes\n", format_bytes(entry.size)));
        out.push_str(&format!("        • Duration    : {}\n", format_duration(entry.duration)));
        if !entry.checksum.is_empty() {
            out.push_str(&format!("        • SHA256      : [ {} ]\n", entry.checksum));
        } else {
            out.push_str("        • SHA256      : [ FATAL: not provided by the repository! ]\n");
        }
        out.push_str(&format!("        • Status      : {}\n", entry.status));
    }
    out.push_str("\n─────────────────────────────────────────────────────────────\n\n");
    out
}

fn write_operation_summary_to_log(entries: &[PackageSummary], log_dir: &Path) -> Option<PathBuf> {
    let path = log_file_path(log_dir);
    let content = render_operation_summary(entries);
    match OpenOptions::new().create(true).append(true).open(&path) {
        Ok(mut file) => {
            if let Err(e) = file.write_all(content.as_bytes()) {
                error(&format!("Failed to write detailed summary to {}: {}", path.display(), e));
                return None;
            }
            Some(path)
        }
        Err(e) => {
            error(&format!("Failed to open {}: {}", path.display(), e));
            None
        }
    }
}

pub fn print_operation_summary(entries: &[PackageSummary], log_path: &Path) {
    let installed = entries
        .iter()
        .filter(|e| e.status == "installed" || e.status == "updated" || e.status == "deleted")
        .count();
    let failed = entries.len() - installed;
    println!("");
    println!("─────────────────────────────────────────────────────────────");
    println!("");
    success("Operation was completed. Short summary:");
    println!("");
    if failed > 0 {
        println!(
            "    {} success, {} failed.",
            installed.to_string().bright_green(),
            failed.to_string().bright_red()
        );
    } else {
        println!("    {} package(s) has been proceed.", installed.to_string().bright_green());
    }
    println!("");
    match write_operation_summary_to_log(entries, log_path) {
        Some(path) => {
            info(&format!("For detailed summary, please see: {}", path.display()));
        }
        None => {
            warning("Failed to save detailed summary! See the above error log.");
        }
    }
    println!("");
    println!("─────────────────────────────────────────────────────────────");
}

pub fn format_bytes(bytes: u64) -> String {
    let grouped = bytes.to_string();
    let mut out = String::new();
    for (i, ch) in grouped.chars().rev().enumerate() {
        if i > 0 && i % 3 == 0 {
            out.push(',');
        }
        out.push(ch);
    }
    out.chars().rev().collect()
}

pub fn format_duration(duration: std::time::Duration) -> String {
    let secs = duration.as_secs();
    let hours = secs / 3600;
    let minutes = (secs % 3600) / 60;
    let seconds = secs % 60;
    if hours > 0 {
        format!("{}h {}m {}s", hours, minutes, seconds)
    } else if minutes > 0 {
        format!("{}m {}s", minutes, seconds)
    } else {
        format!("{}s", seconds)
    }
}
