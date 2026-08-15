use clap::Parser;
use codexbar::{
    cli::{self, Cli, Commands, exit_codes},
    logging, wsl,
};
use std::future::Future;
use std::path::{Path, PathBuf};
use std::time::{Duration, SystemTime};
use tokio::runtime::Runtime;

const LAUNCH_LOG_PREFIX: &str = "codexbar_launch_";
const LAUNCH_LOG_SUFFIX: &str = ".log";
/// Leftover launch logs older than this are swept on the next run. Long enough
/// that someone who hit a failure can still find the log.
const LAUNCH_LOG_MAX_AGE: Duration = Duration::from_secs(24 * 60 * 60);
/// Bound on removals per run, so a temp dir full of leftovers cannot stall a
/// startup. Repeated runs converge.
const LAUNCH_LOG_SWEEP_LIMIT: usize = 512;

fn launch_log_path() -> PathBuf {
    std::env::temp_dir().join(format!(
        "{LAUNCH_LOG_PREFIX}{}{LAUNCH_LOG_SUFFIX}",
        std::process::id()
    ))
}

/// `statusline` is invoked once per editor render, so it must not touch the
/// disk on the way in. It reads cached state only and reports failures through
/// its host, so there is no launch problem a temp log would explain.
fn skips_launch_log(first_arg: Option<&str>) -> bool {
    first_arg == Some("statusline")
}

fn is_launch_log(path: &Path) -> bool {
    path.file_name()
        .and_then(|name| name.to_str())
        .is_some_and(|name| {
            name.starts_with(LAUNCH_LOG_PREFIX) && name.ends_with(LAUNCH_LOG_SUFFIX)
        })
}

/// Truncating open. The OS reuses PIDs, so an append would let one file grow
/// across unrelated runs.
fn start_launch_log(log_path: &Path, message: &str) {
    let _ = std::fs::OpenOptions::new()
        .create(true)
        .write(true)
        .truncate(true)
        .open(log_path)
        .and_then(|mut f| {
            use std::io::Write;
            f.write_all(message.as_bytes())
        });
}

fn append_launch_log(log_path: &Path, message: &str) {
    let _ = std::fs::OpenOptions::new()
        .create(true)
        .append(true)
        .open(log_path)
        .and_then(|mut f| {
            use std::io::Write;
            f.write_all(message.as_bytes())
        });
}

/// Deletes launch logs left behind by runs that failed or were killed. The
/// name check is a cheap string compare, so `metadata` is only paid for actual
/// launch logs rather than for every file in the temp dir.
fn sweep_stale_launch_logs_in(dir: &Path, keep: &Path, now: SystemTime) -> usize {
    let Ok(entries) = std::fs::read_dir(dir) else {
        return 0;
    };
    let mut removed = 0;
    for entry in entries.flatten() {
        if removed >= LAUNCH_LOG_SWEEP_LIMIT {
            break;
        }
        let path = entry.path();
        if path == keep || !is_launch_log(&path) {
            continue;
        }
        let stale = entry
            .metadata()
            .and_then(|metadata| metadata.modified())
            .ok()
            .and_then(|modified| now.duration_since(modified).ok())
            .is_some_and(|age| age > LAUNCH_LOG_MAX_AGE);
        if stale && std::fs::remove_file(&path).is_ok() {
            removed += 1;
        }
    }
    removed
}

fn launch_arg_summary() -> String {
    let arg_count = std::env::args().count().saturating_sub(1);
    format!("{} CLI argument value(s) omitted", arg_count)
}

fn main() {
    let first_arg = std::env::args().nth(1);
    let log_path = (!skips_launch_log(first_arg.as_deref())).then(launch_log_path);

    if let Some(path) = log_path.as_deref() {
        start_launch_log(
            path,
            &format!(
                "main() started at {:?}\nArgs: {:?}\n",
                SystemTime::now(),
                launch_arg_summary()
            ),
        );
        sweep_stale_launch_logs_in(&std::env::temp_dir(), path, SystemTime::now());
    }

    let exit_code = run(log_path.as_deref());

    if let Some(path) = log_path.as_deref() {
        if exit_code == exit_codes::SUCCESS {
            // A clean run has nothing to post-mortem, so it leaves no file
            // behind. Without this every invocation seeds the temp dir forever.
            let _ = std::fs::remove_file(path);
        } else {
            append_launch_log(path, &format!("Exiting with code: {}\n", exit_code));
        }
    }

    std::process::exit(exit_code);
}

fn run(log_path: Option<&Path>) -> i32 {
    if let Some(path) = log_path {
        append_launch_log(path, &startup_log());
    }

    let cli = match Cli::try_parse() {
        Ok(cli) => cli,
        Err(error) => {
            // `Cli::parse()` exits the process itself for `--help`, `--version`,
            // and usage errors, which would skip main()'s cleanup and strand
            // the launch log. None of those are launch failures worth a file.
            if let Some(path) = log_path {
                let _ = std::fs::remove_file(path);
            }
            error.exit();
        }
    };

    if let Err(e) = logging::init(cli.verbose, cli.json_output) {
        eprintln!("Failed to initialize logging: {}", e);
        return exit_codes::UNEXPECTED_FAILURE;
    }

    let rt = match Runtime::new() {
        Ok(rt) => rt,
        Err(e) => {
            eprintln!("Failed to create runtime: {}", e);
            return exit_codes::UNEXPECTED_FAILURE;
        }
    };

    dispatch_command(&rt, cli.command)
}

fn startup_log() -> String {
    let mut log = format!("Starting at {:?}\n", std::time::SystemTime::now());

    if let Some(wsl_log) = wsl_log() {
        log.push_str(&wsl_log);
    }

    log.push_str(&format!("Args: {:?}\n", launch_arg_summary()));
    log
}

fn wsl_log() -> Option<String> {
    if !wsl::is_wsl() {
        return None;
    }

    let mut log = "Running inside WSL\n".to_string();
    if let Some(info) = wsl::get_wsl_info() {
        log.push_str(&format!("  Distro: {}\n", info.distro_name));
        log.push_str(&format!("  Drive mount: {:?}\n", info.drive_mount));
    }

    Some(log)
}

fn dispatch_command(rt: &Runtime, command: Option<Commands>) -> i32 {
    match command {
        Some(Commands::Usage(args)) => run_categorized(rt, cli::usage::run(args)),
        Some(Commands::Cost(args)) => run_categorized(rt, cli::cost::run(args)),
        Some(Commands::Diagnose(args)) => run_categorized(rt, cli::diagnose::run(args)),
        Some(Commands::Sessions(args)) => run_categorized(rt, cli::sessions::run(args)),
        Some(Commands::Serve(args)) => run_unexpected(rt, cli::serve::run(args)),
        Some(Commands::Statusline(args)) => run_unexpected(rt, cli::statusline::run(args)),
        Some(Commands::Mcp(args)) => run_unexpected(rt, cli::mcp::run(args)),
        Some(Commands::Autostart(args)) => run_unexpected(rt, cli::autostart::run(args)),
        Some(Commands::Account(args)) => run_unexpected(rt, cli::account::run(args)),
        Some(Commands::Config(args)) => run_unexpected(rt, cli::config::run(args)),
        None => missing_subcommand(),
    }
}

fn run_categorized<F>(rt: &Runtime, future: F) -> i32
where
    F: Future<Output = anyhow::Result<()>>,
{
    run_command(rt, future, categorize_error)
}

fn run_unexpected<F>(rt: &Runtime, future: F) -> i32
where
    F: Future<Output = anyhow::Result<()>>,
{
    run_command(rt, future, |_| exit_codes::UNEXPECTED_FAILURE)
}

fn run_command<F>(rt: &Runtime, future: F, error_code: fn(&anyhow::Error) -> i32) -> i32
where
    F: Future<Output = anyhow::Result<()>>,
{
    match rt.block_on(future) {
        Ok(()) => exit_codes::SUCCESS,
        Err(e) => {
            eprintln!("Error: {}", e);
            error_code(&e)
        }
    }
}

fn missing_subcommand() -> i32 {
    // The egui menubar shell has been retired; the desktop UI lives in
    // apps/desktop-tauri. The CLI binary now requires an explicit subcommand.
    eprintln!(
        "codexbar is now CLI-only. Run a subcommand (e.g. `codexbar usage -p claude`) \
         or launch the Tauri desktop shell via `apps/desktop-tauri`.\n\
         Use `codexbar --help` for the full list of subcommands."
    );
    exit_codes::USAGE_ERROR
}

/// Categorize an error into the appropriate exit code
fn categorize_error(e: &anyhow::Error) -> i32 {
    let msg = e.to_string().to_lowercase();

    if msg.contains("not installed") || msg.contains("not found") || msg.contains("binary") {
        exit_codes::PROVIDER_MISSING
    } else if msg.contains("parse") || msg.contains("format") || msg.contains("invalid") {
        exit_codes::PARSE_ERROR
    } else if msg.contains("timeout") || msg.contains("timed out") {
        exit_codes::CLI_TIMEOUT
    } else {
        exit_codes::UNEXPECTED_FAILURE
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn launch_log_path_is_process_scoped() {
        let path = launch_log_path();
        let file_name = path.file_name().and_then(|name| name.to_str()).unwrap();

        assert!(file_name.starts_with("codexbar_launch_"));
        assert!(file_name.ends_with(".log"));
        assert!(file_name.contains(&std::process::id().to_string()));
    }

    /// SBS-888: `statusline` runs once per editor render, so it must not write
    /// a temp file on the way in.
    #[test]
    fn only_statusline_skips_the_launch_log() {
        assert!(skips_launch_log(Some("statusline")));
        assert!(!skips_launch_log(Some("usage")));
        assert!(!skips_launch_log(Some("cost")));
        assert!(!skips_launch_log(None));
    }

    #[test]
    fn launch_log_names_are_recognized_without_matching_neighbours() {
        assert!(is_launch_log(Path::new("/tmp/codexbar_launch_42.log")));
        assert!(!is_launch_log(Path::new("/tmp/codexbar_launch_42.txt")));
        assert!(!is_launch_log(Path::new("/tmp/other_launch_42.log")));
        assert!(!is_launch_log(Path::new("/tmp/codexbar.log")));
    }

    /// A reused PID must not append onto a previous run's file.
    #[test]
    fn starting_a_launch_log_truncates_a_reused_path() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("codexbar_launch_1.log");
        std::fs::write(&path, b"stale content from a previous process\n").unwrap();

        start_launch_log(&path, "fresh\n");

        assert_eq!(std::fs::read_to_string(&path).unwrap(), "fresh\n");
    }

    #[test]
    fn sweep_removes_stale_launch_logs_but_spares_mine_and_unrelated_files() {
        let dir = tempfile::tempdir().unwrap();

        let abandoned_a = dir.path().join("codexbar_launch_1.log");
        let abandoned_b = dir.path().join("codexbar_launch_2.log");
        let mine = dir.path().join("codexbar_launch_3.log");
        let unrelated = dir.path().join("something_else.log");
        for path in [&abandoned_a, &abandoned_b, &mine, &unrelated] {
            std::fs::write(path, b"x").unwrap();
        }

        // Age every file past the bound by moving `now` forward, rather than
        // rewriting mtimes through a platform-specific API.
        let later = SystemTime::now() + LAUNCH_LOG_MAX_AGE + Duration::from_secs(60);
        let removed = sweep_stale_launch_logs_in(dir.path(), &mine, later);

        assert_eq!(removed, 2);
        assert!(!abandoned_a.exists());
        assert!(!abandoned_b.exists());
        assert!(mine.exists(), "the current process's log must survive");
        assert!(unrelated.exists(), "unrelated temp files must survive");
    }

    #[test]
    fn sweep_keeps_launch_logs_inside_the_age_bound() {
        let dir = tempfile::tempdir().unwrap();
        let recent = dir.path().join("codexbar_launch_9.log");
        std::fs::write(&recent, b"x").unwrap();

        // Just written, so well inside the 24-hour bound.
        let removed = sweep_stale_launch_logs_in(
            dir.path(),
            &dir.path().join("codexbar_launch_1.log"),
            SystemTime::now(),
        );

        assert_eq!(removed, 0);
        assert!(recent.exists());
    }

    #[test]
    fn sweep_tolerates_a_missing_directory() {
        let dir = tempfile::tempdir().unwrap();
        let missing = dir.path().join("gone");

        assert_eq!(
            sweep_stale_launch_logs_in(&missing, &missing.join("x.log"), SystemTime::now()),
            0
        );
    }
}
