use std::env;
use std::io::{self, IsTerminal};

const RESET: &str = "\x1b[0m";
const BOLD: &str = "\x1b[1m";

const BLUE: &str = "\x1b[38;5;75m";
const BLUE_SOFT: &str = "\x1b[38;5;111m";
const GREY: &str = "\x1b[38;5;245m";
const GREY_LIGHT: &str = "\x1b[38;5;250m";
const YELLOW: &str = "\x1b[38;5;221m";
const RED: &str = "\x1b[38;5;210m";

pub fn print_success(message: &str) {
    println!("{} {}", label("done", BLUE), message);
}

pub fn print_info(message: &str) {
    println!("{} {}", label("info", GREY), message);
}

pub fn print_error_block(message: &str) {
    let lines: Vec<&str> = message.lines().collect();
    let Some(first) = lines.first().copied() else {
        return;
    };

    let command_lines: Vec<&str> = lines
        .iter()
        .copied()
        .filter(|line| line.starts_with("burner ") || line.starts_with("usage: burner "))
        .collect();

    if let Some(usage) = first.strip_prefix("usage: ") {
        eprintln!("{} {}", label_err("error", RED), format_cli_line(usage, true));
    } else if command_lines.is_empty() {
        eprintln!("{} {}", label_err("error", RED), first);
        return;
    } else {
        eprintln!("{} {}", label_err("error", RED), first);
    }

    if command_lines.is_empty() {
        return;
    }

    eprintln!();
    eprintln!("  {}", paint_stderr("---", GREY));
    eprintln!("  {}", paint_stderr("| Commands available", GREY));
    eprintln!("  {}", paint_stderr("---", GREY));
    for line in command_lines {
        let command = line.strip_prefix("usage: ").unwrap_or(line);
        let bullet = paint_stderr(" |-", GREY);
        let star = bold_stderr("✦", BLUE);
        eprintln!("  {bullet} {star} {}", format_cli_line(command, true));
    }
    eprintln!();
}

pub fn print_muted(message: &str) {
    println!("{}", paint_stdout(message, GREY));
}

pub fn colorize_header(value: &str) -> String {
    bold_stdout(value, BLUE)
}

pub fn colorize_runtime(value: &str) -> String {
    match value {
        "local" => bold_stdout(value, BLUE),
        "systemd" => bold_stdout(value, GREY_LIGHT),
        "-" => paint_stdout(value, GREY),
        _ => bold_stdout(value, BLUE_SOFT),
    }
}

pub fn colorize_status(value: &str) -> String {
    match value {
        "running" | "active" => bold_stdout(value, BLUE),
        "stopped" | "inactive" => paint_stdout(value, GREY),
        "pending" | "installed" | "activating" => bold_stdout(value, GREY_LIGHT),
        "restarting" | "exited" => bold_stdout(value, YELLOW),
        "failed" | "dead" => bold_stdout(value, RED),
        "-" => paint_stdout(value, GREY),
        _ => bold_stdout(value, BLUE_SOFT),
    }
}

pub fn colorize_name(value: &str) -> String {
    bold_stdout(value, BLUE)
}

pub fn colorize_pid(value: &str) -> String {
    if value == "0" || value == "-" {
        paint_stdout(value, GREY)
    } else {
        paint_stdout(value, BLUE_SOFT)
    }
}

pub fn colorize_location(value: &str) -> String {
    paint_stdout(value, GREY_LIGHT)
}

pub fn colorize_secret(value: &str) -> String {
    bold_stdout(value, BLUE_SOFT)
}

pub fn colorize_border(value: &str) -> String {
    paint_stdout(value, GREY)
}

pub fn pad_right(styled: String, visible_width: usize, width: usize) -> String {
    let padding = width.saturating_sub(visible_width);
    if padding == 0 {
        styled
    } else {
        format!("{styled}{}", " ".repeat(padding))
    }
}

fn label(text: &str, color: &str) -> String {
    if use_stdout_color() {
        format!("{color}{BOLD}● {text}{RESET}")
    } else {
        format!("[{text}]")
    }
}

fn label_err(text: &str, color: &str) -> String {
    if use_stderr_color() {
        format!("{color}{BOLD}● {text}{RESET}")
    } else {
        format!("[{text}]")
    }
}

fn bold_stderr(text: &str, color: &str) -> String {
    if use_stderr_color() {
        format!("{color}{BOLD}{text}{RESET}")
    } else {
        text.to_string()
    }
}

fn paint_stderr(text: &str, color: &str) -> String {
    if use_stderr_color() {
        format!("{color}{text}{RESET}")
    } else {
        text.to_string()
    }
}

fn bold_stdout(text: &str, color: &str) -> String {
    if use_stdout_color() {
        format!("{color}{BOLD}{text}{RESET}")
    } else {
        text.to_string()
    }
}

fn paint_stdout(text: &str, color: &str) -> String {
    if use_stdout_color() {
        format!("{color}{text}{RESET}")
    } else {
        text.to_string()
    }
}

fn format_cli_line(line: &str, stderr: bool) -> String {
    line.split_whitespace()
        .map(|token| style_cli_token(token, stderr))
        .collect::<Vec<_>>()
        .join(" ")
}

fn style_cli_token(token: &str, stderr: bool) -> String {
    if token == "burner" {
        return paint(token, BLUE, stderr, true);
    }

    if token.starts_with('-') || token.starts_with("[--") {
        return paint(token, BLUE_SOFT, stderr, true);
    }

    if token.contains("<") && token.contains(">") {
        return paint(token, GREY, stderr, true);
    }

    if token.starts_with('"') || token.ends_with('"') {
        return paint(token, GREY_LIGHT, stderr, false);
    }

    if matches!(
        token,
        "online"
            | "daemon"
            | "serve"
            | "link"
            | "deploy"
            | "run"
            | "list"
            | "logs"
            | "start"
            | "stop"
            | "restart"
    ) {
        return paint(token, BLUE, stderr, true);
    }

    paint(token, GREY_LIGHT, stderr, false)
}

fn paint(text: &str, color: &str, stderr: bool, bold: bool) -> String {
    match (stderr, bold) {
        (true, true) => bold_stderr(text, color),
        (true, false) => paint_stderr(text, color),
        (false, true) => bold_stdout(text, color),
        (false, false) => paint_stdout(text, color),
    }
}

fn use_stdout_color() -> bool {
    colors_enabled() && io::stdout().is_terminal()
}

fn use_stderr_color() -> bool {
    colors_enabled() && io::stderr().is_terminal()
}

fn colors_enabled() -> bool {
    env::var_os("NO_COLOR").is_none() && env::var("TERM").unwrap_or_default() != "dumb"
}
