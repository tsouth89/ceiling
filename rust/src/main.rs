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
/// Launch logs live in their own directory under temp. Sweeping the temp root
/// would mean a name check on every entry, and Windows `%TEMP%` routinely holds
/// tens of thousands of installer and browser files.
const LAUNCH_LOG_DIR: &str = "codexbar-launch-logs";
/// Leftover launch logs older than this are swept on the next run. Long enough
/// that someone who hit a failure can still find the log.
const LAUNCH_LOG_MAX_AGE: Duration = Duration::from_secs(24 * 60 * 60);
/// Bound on removals per run, so a directory full of leftovers cannot stall a
/// startup. Repeated runs converge.
const LAUNCH_LOG_SWEEP_LIMIT: usize = 512;

/// The launch-log directory, created if missing.
///
/// Returns `None` when the path is not a real directory. On a shared `/tmp`
/// the name could be a symlink someone else planted to redirect our writes and
/// our sweep, so anything but a true directory is refused.
fn launch_log_dir_in(temp_dir: &Path) -> Option<PathBuf> {
    let dir = temp_dir.join(LAUNCH_LOG_DIR);
    std::fs::create_dir_all(&dir).ok()?;
    // `symlink_metadata` does not follow, so a symlink reports `is_dir()` false.
    std::fs::symlink_metadata(&dir)
        .ok()
        .filter(std::fs::Metadata::is_dir)
        .map(|_| dir)
}

fn launch_log_path() -> Option<PathBuf> {
    let dir = launch_log_dir_in(&std::env::temp_dir())?;
    Some(dir.join(format!(
        "{LAUNCH_LOG_PREFIX}{}{LAUNCH_LOG_SUFFIX}",
        std::process::id()
    )))
}

/// `statusline` is invoked once per editor render, so it must not touch the
/// disk on the way in. It reads cached state only and reports failures through
/// its host, so there is no launch problem a temp log would explain.
///
/// Scans every argument rather than just the first: clap accepts the global
/// flags before the subcommand, so `codexbar --verbose statusline` is the same
/// per-render path. No global flag takes `statusline` as a value (`--log-level`
/// is restricted to log levels), so a bare scan cannot misfire on one.
fn skips_launch_log<I, S>(args: I) -> bool
where
    I: IntoIterator<Item = S>,
    S: AsRef<str>,
{
    args.into_iter().any(|arg| arg.as_ref() == "statusline")
}

fn is_launch_log(path: &Path) -> bool {
    path.file_name()
        .and_then(|name| name.to_str())
        .is_some_and(|name| {
            name.starts_with(LAUNCH_LOG_PREFIX) && name.ends_with(LAUNCH_LOG_SUFFIX)
        })
}

/// Starts a run's log, replacing whatever the last holder of this PID left.
///
/// Uses `create_new` rather than a truncating open. PIDs are predictable, so on
/// a shared temp dir someone can pre-create this name as a symlink to a file
/// they want destroyed; a truncating open would follow it and empty the target.
/// `create_new` is `O_CREAT | O_EXCL`, which refuses any existing entry
/// including a symlink, so the worst a planted link can do is cost us the log.
fn start_launch_log(log_path: &Path, message: &str) {
    if std::fs::symlink_metadata(log_path).is_ok() {
        // Removing a symlink unlinks the link, never the target.
        let _ = std::fs::remove_file(log_path);
    }
    let _ = std::fs::OpenOptions::new()
        .create_new(true)
        .write(true)
        .open(log_path)
        .and_then(|mut f| {
            use std::io::Write;
            f.write_all(message.as_bytes())
        });
}

/// Appends to a log [`start_launch_log`] already created. Deliberately does not
/// create: if the start failed, the entry at this path is not ours to write to.
fn append_launch_log(log_path: &Path, message: &str) {
    let _ = std::fs::OpenOptions::new()
        .append(true)
        .open(log_path)
        .and_then(|mut f| {
            use std::io::Write;
            f.write_all(message.as_bytes())
        });
}

/// Deletes launch logs left behind by runs that failed or were killed. Scoped
/// to the launch-log directory, so this never walks the temp root.
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
    let log_path = (!skips_launch_log(std::env::args().skip(1)))
        .then(launch_log_path)
        .flatten();

    if let Some(path) = log_path.as_deref() {
        start_launch_log(
            path,
            &format!(
                "main() started at {:?}\nArgs: {:?}\n",
                SystemTime::now(),
                launch_arg_summary()
            ),
        );
        if let Some(dir) = path.parent() {
            sweep_stale_launch_logs_in(dir, path, SystemTime::now());
        }
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
    fn launch_log_path_is_process_scoped_and_inside_its_own_directory() {
        let path = launch_log_path().expect("launch log path");
        let file_name = path.file_name().and_then(|name| name.to_str()).unwrap();

        assert!(file_name.starts_with("codexbar_launch_"));
        assert!(file_name.ends_with(".log"));
        assert!(file_name.contains(&std::process::id().to_string()));
        assert_eq!(
            path.parent().and_then(|dir| dir.file_name()),
            Some(std::ffi::OsStr::new(LAUNCH_LOG_DIR)),
            "logs must live in their own directory so the sweep is scoped"
        );
    }

    /// SBS-888: `statusline` runs once per editor render, so it must not write
    /// a temp file on the way in. Clap takes the global flags before the
    /// subcommand, so those spellings are the same per-render path.
    #[test]
    fn statusline_skips_the_launch_log_behind_global_flags() {
        assert!(skips_launch_log(["statusline"]));
        assert!(skips_launch_log(["--verbose", "statusline"]));
        assert!(skips_launch_log(["--no-color", "statusline"]));
        assert!(skips_launch_log(["--log-level", "info", "statusline"]));
        assert!(skips_launch_log(["--json-output", "statusline"]));

        assert!(!skips_launch_log(["usage"]));
        assert!(!skips_launch_log(["cost", "--days", "7"]));
        assert!(!skips_launch_log(["--verbose", "diagnose"]));
        assert!(!skips_launch_log(Vec::<&str>::new()));
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
    fn starting_a_launch_log_replaces_a_reused_path() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("codexbar_launch_1.log");
        std::fs::write(&path, b"stale content from a previous process\n").unwrap();

        start_launch_log(&path, "fresh\n");

        assert_eq!(std::fs::read_to_string(&path).unwrap(), "fresh\n");
    }

    /// PIDs are predictable, so on a shared `/tmp` someone can pre-create the
    /// next run's log name as a symlink to a file they want destroyed. Starting
    /// a log must never write through it.
    #[cfg(unix)]
    #[test]
    fn starting_a_launch_log_does_not_write_through_a_planted_symlink() {
        let dir = tempfile::tempdir().unwrap();
        let victim = dir.path().join("precious.txt");
        std::fs::write(&victim, b"do not lose me").unwrap();
        let path = dir.path().join("codexbar_launch_1.log");
        std::os::unix::fs::symlink(&victim, &path).unwrap();

        start_launch_log(&path, "fresh\n");

        assert_eq!(
            std::fs::read_to_string(&victim).unwrap(),
            "do not lose me",
            "the symlink target must be untouched"
        );
        assert!(
            !std::fs::symlink_metadata(&path)
                .unwrap()
                .file_type()
                .is_symlink(),
            "the planted link should have been unlinked, not followed"
        );
    }

    /// Appending is for a log this run created. It must not resurrect a path
    /// that `start_launch_log` refused.
    #[test]
    fn appending_does_not_create_a_missing_log() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("codexbar_launch_1.log");

        append_launch_log(&path, "should not appear\n");

        assert!(!path.exists());
    }

    /// A symlink where the log directory should be means someone is trying to
    /// redirect our writes and our sweep.
    #[cfg(unix)]
    #[test]
    fn a_symlinked_log_directory_is_refused() {
        let dir = tempfile::tempdir().unwrap();
        let elsewhere = dir.path().join("elsewhere");
        std::fs::create_dir(&elsewhere).unwrap();
        std::os::unix::fs::symlink(&elsewhere, dir.path().join(LAUNCH_LOG_DIR)).unwrap();

        assert!(launch_log_dir_in(dir.path()).is_none());
    }

    #[test]
    fn the_log_directory_is_created_when_missing() {
        let dir = tempfile::tempdir().unwrap();

        let created = launch_log_dir_in(dir.path()).expect("log dir");

        assert!(created.is_dir());
        assert_eq!(created, dir.path().join(LAUNCH_LOG_DIR));
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
