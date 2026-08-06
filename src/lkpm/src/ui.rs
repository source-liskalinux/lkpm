use colored::*;
use std::io::{self, Write};

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
        println!("    > {} ({})", dep, status);
    }
    for dep in optional.iter() {
        println!("    > {} ({})", dep, "optional".bright_yellow());
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
        println!("    > {}", dep);
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
        println!("    > {}", dep);
    }
}

pub fn confirm(prompt: &str, default: bool) -> bool {
    let def = if default { "y" } else { "n" };
    print!("{} {} {}", "[?]".bright_yellow(), prompt.bright_yellow(), format!("[y/n] (default: {})\n    > ", def).bright_yellow());
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

pub fn print_operation_summary(entries: &[PackageSummary]) {
    println!("");
    println!("─────────────────────────────────────────────────────────────");
    success("Operation was completed. Summary:");
    for entry in entries.iter() {
        if entry.status == "installed" || entry.status == "updated" || entry.status == "deleted" {
            println!("");
            sum_success(&format!("          ::: [ {} ({}) ] :::", entry.name, entry.version));
            sum_success(&format!("        > Source      : {}", entry.source));
            sum_success(&format!("        > Size        : {} bytes", format_bytes(entry.size)));
            sum_success(&format!("        > Duration    : {}", format_duration(entry.duration)));
            if !entry.checksum.is_empty() {
                sum_success(&format!("        > SHA256      : [ {} ]", entry.checksum));
            } else {
                sum_success(&format!("{} {}", "        > SHA256      :", "[ FATAL: not provided by the repository! ]".bright_yellow()));
            }
            sum_success(&format!("        > Status      : {}", entry.status));
        } else {
            println!("");
            sum_error(&format!("          ::: [ {} ({}) ] :::", entry.name, entry.version));
            sum_error(&format!("        > Source      : {}", entry.source));
            sum_error(&format!("        > Size        : {} bytes", format_bytes(entry.size)));
            sum_error(&format!("        > Duration    : {}", format_duration(entry.duration)));
            if !entry.checksum.is_empty() {
                sum_error(&format!("        > SHA256      : [ {} ]", entry.checksum));
            } else {
                sum_error(&format!("{} {}", "        > SHA256      :", "[ FATAL: not provided by the repository! ]".bright_yellow()));
            }
            sum_error(&format!("        > Status      : {}", entry.status));
        }
    }
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
