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
/// Launch logs live under the per-user cache directory, not the system temp
/// dir.
///
/// `/tmp` is shared and world-writable, and the file name is a predictable PID.
/// Anyone on the box could pre-create the next run's name (or squat the parent
/// directory) and steer our writes at a file of their choosing. `dirs::cache_dir()`
/// is inside the user's own home, which removes most of that exposure rather
/// than guarding it; the owner and mode checks below cover what is left (an
/// exported or mis-permissioned home, `XDG_CACHE_HOME` pointed somewhere
/// shared). It also means the sweep never walks a directory holding thousands
/// of unrelated files.
const LAUNCH_LOG_DIR: &str = "launch-logs";
/// Records that the one-time pass over the old system-temp location is done, so
/// steady-state runs never read_dir the temp root. Lives in our own directory
/// and is not named like a launch log, so the sweep leaves it alone.
const LEGACY_SWEEP_MARKER: &str = ".legacy-temp-swept";
/// Leftover launch logs older than this are swept on the next run. Long enough
/// that someone who hit a failure can still find the log.
const LAUNCH_LOG_MAX_AGE: Duration = Duration::from_secs(24 * 60 * 60);
/// Bound on removals per run, so a directory full of leftovers cannot stall a
/// startup. Repeated runs converge.
const LAUNCH_LOG_SWEEP_LIMIT: usize = 512;
/// Bound on directory entries examined per run. A removal cap alone still walks
/// every entry, and the name check is paid on all of them.
const LAUNCH_LOG_SCAN_LIMIT: usize = 4096;

/// How a `stat` of the launch-log directory should be read.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum LogDirVerdict {
    /// Ours and private. Safe to write in.
    Usable,
    /// Ours, but the mode lets other accounts in. Tighten, then re-check.
    NeedsTightening,
    /// A symlink, a non-directory, or another account's. Nothing to repair, so
    /// this run logs nowhere.
    Refuse,
}

/// The rule for accepting a launch-log directory, split from the filesystem so
/// it is testable on every platform (shared-crate CI is Windows-only, where the
/// unix branch never runs).
///
/// A directory another account owns can hold planted per-PID symlinks, can be
/// read by that account, or - if a `sudo` first run created it - can lock every
/// later unprivileged run out of its own log.
fn classify_log_dir(is_dir: bool, owned_by_us: bool, mode: u32) -> LogDirVerdict {
    if !is_dir || !owned_by_us {
        return LogDirVerdict::Refuse;
    }
    // `mode` is the raw `st_mode`, so mask off the file-type bits.
    if mode & 0o077 != 0 {
        return LogDirVerdict::NeedsTightening;
    }
    LogDirVerdict::Usable
}

/// Creates the launch-log directory private from the start, so there is no
/// window where it exists with a permissive mode.
fn create_log_dir(dir: &Path) -> std::io::Result<()> {
    if let Some(parent) = dir.parent() {
        // The parent (`<cache>/Ceiling`) is shared with other caches, so it
        // keeps the default mode.
        std::fs::create_dir_all(parent)?;
    }
    let mut builder = std::fs::DirBuilder::new();
    builder.recursive(false);
    #[cfg(unix)]
    {
        use std::os::unix::fs::DirBuilderExt;
        builder.mode(0o700);
    }
    match builder.create(dir) {
        Err(error) if error.kind() == std::io::ErrorKind::AlreadyExists => Ok(()),
        other => other,
    }
}

#[cfg(unix)]
fn current_uid() -> u32 {
    // SAFETY: `getuid` reads no memory, cannot fail, and has no side effects.
    unsafe { libc::getuid() }
}

/// Refuses a launch-log directory we do not fully control, repairing the one
/// case that is ours to repair.
#[cfg(unix)]
fn accept_log_dir(dir: &Path) -> Option<()> {
    use std::os::unix::fs::{MetadataExt, PermissionsExt};

    // `symlink_metadata` does not follow, so a symlink reports `is_dir()` false
    // and is refused before anything below can be redirected through it.
    let metadata = std::fs::symlink_metadata(dir).ok()?;
    match classify_log_dir(
        metadata.is_dir(),
        metadata.uid() == current_uid(),
        metadata.mode(),
    ) {
        LogDirVerdict::Usable => Some(()),
        LogDirVerdict::Refuse => None,
        LogDirVerdict::NeedsTightening => {
            std::fs::set_permissions(dir, std::fs::Permissions::from_mode(0o700)).ok()?;
            let after = std::fs::symlink_metadata(dir).ok()?;
            (classify_log_dir(after.is_dir(), after.uid() == current_uid(), after.mode())
                == LogDirVerdict::Usable)
                .then_some(())
        }
    }
}

/// Windows has no equivalent of the shared-`/tmp` squat this guards against:
/// the cache directory sits under the per-user `%LOCALAPPDATA%`, whose default
/// ACL already excludes other accounts, and there is no owner or mode to
/// re-check. So the ownership and mode inputs are the answers that directory
/// already gives, and confirming it is a real directory is the whole check.
#[cfg(not(unix))]
fn accept_log_dir(dir: &Path) -> Option<()> {
    let is_dir = std::fs::symlink_metadata(dir).is_ok_and(|metadata| metadata.is_dir());
    (classify_log_dir(is_dir, true, 0o700) == LogDirVerdict::Usable).then_some(())
}

/// The launch-log directory, created if missing, or `None` when what is on disk
/// is not somewhere we can safely write.
fn launch_log_dir_in(cache_dir: &Path) -> Option<PathBuf> {
    let dir = cache_dir.join("Ceiling").join(LAUNCH_LOG_DIR);
    create_log_dir(&dir).ok()?;
    accept_log_dir(&dir)?;
    Some(dir)
}

fn launch_log_path() -> Option<PathBuf> {
    let dir = launch_log_dir_in(&dirs::cache_dir()?)?;
    Some(dir.join(format!(
        "{LAUNCH_LOG_PREFIX}{}{LAUNCH_LOG_SUFFIX}",
        std::process::id()
    )))
}

/// `statusline` is invoked once per editor render, so it must not touch the
/// disk on the way in. It reads cached state only and reports failures through
/// its host, so there is no launch problem a log would explain.
///
/// Resolves the subcommand with clap itself rather than reading argv by hand.
/// Neither shortcut is correct: `argv[1]` misses the global flags clap accepts
/// before the subcommand (`--verbose statusline`), and a bare scan for the word
/// misreads it as the value of `--provider`, `--format`, `--source` and friends,
/// skipping the log for the failing run that most needs one. A hand-written
/// scanner needs a list of value-taking flags kept in step with `Cli`; clap
/// already has it.
///
/// `try_parse_from` does not exit the process the way `parse` does, and anything
/// it rejects - `--help`, `--version`, a typo - falls through to writing a log,
/// which is the safe direction.
fn skips_launch_log<I, T>(argv: I) -> bool
where
    I: IntoIterator<Item = T>,
    T: Into<std::ffi::OsString> + Clone,
{
    matches!(
        Cli::try_parse_from(argv).ok().and_then(|cli| cli.command),
        Some(Commands::Statusline(_))
    )
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
/// Returns whether the file is now ours to write. `main` drops the path when it
/// is not: handing it on would have [`append_launch_log`] open whatever is
/// sitting there instead.
///
/// Uses `create_new` rather than a truncating open. PIDs are predictable, so
/// anywhere another account can write, someone can pre-create this name as a
/// symlink to a file they want destroyed; a truncating open would follow it and
/// empty the target. `create_new` is `O_CREAT | O_EXCL`, which refuses any
/// existing entry including a symlink, and `O_NOFOLLOW` says the same thing a
/// second way. The worst a planted link can do is cost us the log.
fn start_launch_log(log_path: &Path, message: &str) -> bool {
    if std::fs::symlink_metadata(log_path).is_ok() {
        // Removing a symlink unlinks the link, never the target.
        let _ = std::fs::remove_file(log_path);
    }
    let mut options = std::fs::OpenOptions::new();
    options.create_new(true).write(true);
    #[cfg(unix)]
    {
        use std::os::unix::fs::OpenOptionsExt;
        options.mode(0o600).custom_flags(libc::O_NOFOLLOW);
    }
    let Ok(mut file) = options.open(log_path) else {
        return false;
    };
    use std::io::Write;
    let _ = file.write_all(message.as_bytes());
    true
}

/// Appends to a log [`start_launch_log`] already created.
///
/// Refuses to create, and refuses to follow a link: if the start could not
/// replace what was at this path, the entry is not ours and appending would
/// write the launch header into whatever it points at. `main` already drops the
/// path in that case; this is the second lock on the same door.
///
/// The `is_file` check is not redundant with `O_NOFOLLOW`: it also turns away a
/// planted FIFO, which opening for write would block on.
fn append_launch_log(log_path: &Path, message: &str) {
    let is_regular_file = std::fs::symlink_metadata(log_path)
        .map(|metadata| metadata.is_file())
        .unwrap_or(false);
    if !is_regular_file {
        return;
    }
    let mut options = std::fs::OpenOptions::new();
    options.append(true);
    #[cfg(unix)]
    {
        use std::os::unix::fs::OpenOptionsExt;
        options.custom_flags(libc::O_NOFOLLOW);
    }
    let _ = options.open(log_path).and_then(|mut f| {
        use std::io::Write;
        f.write_all(message.as_bytes())
    });
}

/// What one sweep pass did.
struct SweepOutcome {
    removed: usize,
    /// Launch logs still on disk when the pass ended, including the ones the
    /// age bound spared.
    left: usize,
    /// The pass reached the end of the directory instead of stopping on a cap.
    complete: bool,
}

/// Deletes launch logs left behind by runs that failed or were killed.
///
/// `scan_limit` caps the entries examined, not only the removals. A removal cap
/// alone still walks every entry and pays the name check on each, which is the
/// cost this is meant to keep off the startup path.
///
/// `symlink_metadata` does not follow, so a planted link is neither aged
/// through nor removed as if it were one of ours.
fn sweep_launch_logs_in(
    dir: &Path,
    keep: Option<&Path>,
    now: SystemTime,
    scan_limit: usize,
    remove_limit: usize,
) -> SweepOutcome {
    let mut outcome = SweepOutcome {
        removed: 0,
        left: 0,
        complete: true,
    };
    let Ok(entries) = std::fs::read_dir(dir) else {
        // Nothing readable means nothing left to find on a later pass either.
        return outcome;
    };
    for (scanned, entry) in entries.flatten().enumerate() {
        if scanned >= scan_limit || outcome.removed >= remove_limit {
            outcome.complete = false;
            break;
        }
        let path = entry.path();
        if Some(path.as_path()) == keep || !is_launch_log(&path) {
            continue;
        }
        let Ok(metadata) = std::fs::symlink_metadata(&path) else {
            outcome.left += 1;
            continue;
        };
        if !metadata.is_file() {
            outcome.left += 1;
            continue;
        }
        let stale = metadata
            .modified()
            .ok()
            .and_then(|modified| now.duration_since(modified).ok())
            .is_some_and(|age| age > LAUNCH_LOG_MAX_AGE);
        if stale && std::fs::remove_file(&path).is_ok() {
            outcome.removed += 1;
        } else {
            outcome.left += 1;
        }
    }
    outcome
}

/// The per-run sweep of our own directory, scoped so this never walks the temp
/// root.
fn sweep_stale_launch_logs_in(dir: &Path, keep: &Path, now: SystemTime) -> usize {
    sweep_launch_logs_in(
        dir,
        Some(keep),
        now,
        LAUNCH_LOG_SCAN_LIMIT,
        LAUNCH_LOG_SWEEP_LIMIT,
    )
    .removed
}

/// Removes launch logs earlier versions wrote straight into the system temp
/// root, which is no longer where this one writes.
///
/// Those files are the population that motivated SBS-888, so leaving them would
/// mean the accumulated mess is never cleaned. But the temp root is exactly the
/// directory this change exists to stop walking - a Windows `%TEMP%` routinely
/// holds tens of thousands of installer and browser files - so the pass retires
/// itself with a marker as soon as no legacy log is left. A run that stops on
/// the removal cap, or that finds logs still inside the age bound, writes no
/// marker and tries again next time, so it converges instead of stranding
/// files behind whatever the directory happens to list first.
fn sweep_legacy_temp_launch_logs(log_dir: &Path, temp_dir: &Path, now: SystemTime) {
    let marker = log_dir.join(LEGACY_SWEEP_MARKER);
    if marker.exists() {
        return;
    }
    let outcome = sweep_launch_logs_in(
        temp_dir,
        None,
        now,
        // No scan cap: this runs a handful of times in the life of an install,
        // and a partial walk is what would strand files.
        usize::MAX,
        LAUNCH_LOG_SWEEP_LIMIT,
    );
    if outcome.complete && outcome.left == 0 {
        let _ = std::fs::File::create(&marker);
    }
}

fn launch_arg_summary() -> String {
    let arg_count = std::env::args().count().saturating_sub(1);
    format!("{} CLI argument value(s) omitted", arg_count)
}

/// Whether an exit code is worth keeping a launch log for.
///
/// Only a genuine failure is. A usage error is the user's typo, already
/// explained on stderr, and `missing_subcommand` reaches here through the `Ok`
/// path rather than clap's own exit, so bare `codexbar` would otherwise leave a
/// file behind on every run.
fn keeps_launch_log(exit_code: i32) -> bool {
    exit_code != exit_codes::SUCCESS && exit_code != exit_codes::USAGE_ERROR
}

fn main() {
    let candidate = (!skips_launch_log(std::env::args_os()))
        .then(launch_log_path)
        .flatten();

    let mut started = false;
    if let Some(path) = candidate.as_deref() {
        started = start_launch_log(
            path,
            &format!(
                "main() started at {:?}\nArgs: {:?}\n",
                SystemTime::now(),
                launch_arg_summary()
            ),
        );
        if let Some(dir) = path.parent() {
            sweep_stale_launch_logs_in(dir, path, SystemTime::now());
            sweep_legacy_temp_launch_logs(dir, &std::env::temp_dir(), SystemTime::now());
        }
    }

    // A log we could not create is not ours. Something else is at that path -
    // on a shared directory, a symlink someone planted for this PID - and every
    // later write would land in it.
    let log_path = candidate.filter(|_| started);

    let exit_code = run(log_path.as_deref());

    if let Some(path) = log_path.as_deref() {
        if keeps_launch_log(exit_code) {
            append_launch_log(path, &format!("Exiting with code: {}\n", exit_code));
        } else {
            // Nothing to post-mortem, so leave no file behind. Without this
            // every invocation seeds the log directory forever.
            let _ = std::fs::remove_file(path);
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
    fn launch_log_path_is_process_scoped_and_outside_the_shared_temp_dir() {
        let path = launch_log_path().expect("launch log path");
        let file_name = path.file_name().and_then(|name| name.to_str()).unwrap();

        assert!(file_name.starts_with("codexbar_launch_"));
        assert!(file_name.ends_with(".log"));
        assert!(file_name.contains(&std::process::id().to_string()));
        assert_eq!(
            path.parent().and_then(|dir| dir.file_name()),
            Some(std::ffi::OsStr::new(LAUNCH_LOG_DIR))
        );
        assert!(
            !path.starts_with(std::env::temp_dir()),
            "the shared temp dir is world-writable with a predictable name"
        );
    }

    /// SBS-888: `statusline` runs once per editor render, so it must not write
    /// a file on the way in. Clap takes the global flags before the subcommand,
    /// so those spellings are the same per-render path and `argv[1]` alone
    /// cannot tell.
    #[test]
    fn statusline_skips_the_launch_log_behind_global_flags() {
        assert!(skips_launch_log(["codexbar", "statusline"]));
        assert!(skips_launch_log(["codexbar", "--verbose", "statusline"]));
        assert!(skips_launch_log(["codexbar", "--no-color", "statusline"]));
        assert!(skips_launch_log([
            "codexbar",
            "--log-level",
            "info",
            "statusline"
        ]));
        assert!(skips_launch_log([
            "codexbar",
            "--log-level=info",
            "statusline"
        ]));
        assert!(skips_launch_log([
            "codexbar",
            "--json-output",
            "statusline"
        ]));
        assert!(skips_launch_log([
            "codexbar",
            "--verbose",
            "statusline",
            "--provider",
            "claude"
        ]));

        assert!(!skips_launch_log(["codexbar", "usage"]));
        assert!(!skips_launch_log(["codexbar", "cost", "--days", "7"]));
        assert!(!skips_launch_log(["codexbar", "--verbose", "diagnose"]));
        assert!(!skips_launch_log(["codexbar"]));
    }

    /// `statusline` as the *value* of a flag is a failing usage run, and its
    /// log is exactly the one worth keeping. A bare scan of argv for the token
    /// would throw that log away.
    #[test]
    fn a_flag_value_named_statusline_does_not_skip_the_log() {
        assert!(!skips_launch_log(["codexbar", "--provider", "statusline"]));
        assert!(!skips_launch_log(["codexbar", "-p", "statusline"]));
        assert!(!skips_launch_log([
            "codexbar",
            "usage",
            "--provider",
            "statusline"
        ]));
        assert!(!skips_launch_log(["codexbar", "--format", "statusline"]));
        assert!(!skips_launch_log(["codexbar", "--source", "statusline"]));
        assert!(!skips_launch_log(["codexbar", "--", "statusline"]));
    }

    /// Anything clap rejects - a typo, `--help`, `--version` - falls through to
    /// writing a log. Erring toward a file is the safe direction; `run` deletes
    /// it again on the way out.
    #[test]
    fn an_invocation_clap_cannot_parse_still_gets_a_log() {
        assert!(!skips_launch_log(["codexbar", "statusline", "--nonsense"]));
        assert!(!skips_launch_log(["codexbar", "statusliner"]));
        assert!(!skips_launch_log(["codexbar", "--help"]));
        assert!(!skips_launch_log(["codexbar", "--version"]));
    }

    /// A usage error is the user's typo, already explained on stderr. Bare
    /// `codexbar` returns it through the `Ok` path, not clap's own exit.
    #[test]
    fn only_real_failures_keep_their_log() {
        assert!(!keeps_launch_log(exit_codes::SUCCESS));
        assert!(!keeps_launch_log(exit_codes::USAGE_ERROR));
        assert!(keeps_launch_log(exit_codes::UNEXPECTED_FAILURE));
        assert!(keeps_launch_log(exit_codes::PROVIDER_MISSING));
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

        assert!(start_launch_log(&path, "fresh\n"));

        assert_eq!(std::fs::read_to_string(&path).unwrap(), "fresh\n");
    }

    /// A log we could not create is not ours. `main` reads the `false` and
    /// stops passing the path around, so nothing appends to it later.
    #[test]
    fn starting_a_launch_log_reports_a_path_it_could_not_create() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("no-such-dir").join("codexbar_launch_1.log");

        assert!(!start_launch_log(&path, "fresh\n"));
        assert!(!path.exists());
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

        assert!(start_launch_log(&path, "fresh\n"));

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

    /// The case the review called out: the link is planted in a directory we
    /// cannot write, so the unlink fails too. `start_launch_log` must say so
    /// rather than leave `main` holding a path that points at someone's file.
    #[cfg(unix)]
    #[test]
    fn a_planted_symlink_we_cannot_unlink_turns_the_log_off() {
        use std::os::unix::fs::PermissionsExt;

        // Root ignores the directory mode, so there is nothing to prove here.
        if current_uid() == 0 {
            return;
        }

        let dir = tempfile::tempdir().unwrap();
        let victim = dir.path().join("precious.txt");
        std::fs::write(&victim, b"do not lose me").unwrap();
        let squatted = dir.path().join("squatted");
        std::fs::create_dir(&squatted).unwrap();
        let path = squatted.join("codexbar_launch_1.log");
        std::os::unix::fs::symlink(&victim, &path).unwrap();
        // Read and execute only: the entry can be seen but not replaced.
        std::fs::set_permissions(&squatted, std::fs::Permissions::from_mode(0o500)).unwrap();

        let started = start_launch_log(&path, "launch header\n");
        // Whatever the assertions do, the temp dir has to be removable.
        std::fs::set_permissions(&squatted, std::fs::Permissions::from_mode(0o700)).unwrap();

        assert!(!started, "a log we could not create is not ours to write");
        assert_eq!(
            std::fs::read_to_string(&victim).unwrap(),
            "do not lose me",
            "neither the start nor a later append may reach the target"
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

    /// If `start_launch_log` could not replace a planted link (the victim
    /// cannot unlink a file in a directory someone else owns), the later
    /// appends must not write the header into the target either.
    #[cfg(unix)]
    #[test]
    fn appending_does_not_write_through_a_planted_symlink() {
        let dir = tempfile::tempdir().unwrap();
        let victim = dir.path().join("precious.txt");
        std::fs::write(&victim, b"do not lose me").unwrap();
        let path = dir.path().join("codexbar_launch_1.log");
        std::os::unix::fs::symlink(&victim, &path).unwrap();

        append_launch_log(&path, "leaked header\n");

        assert_eq!(std::fs::read_to_string(&victim).unwrap(), "do not lose me");
    }

    /// A symlink where the log directory should be means someone is trying to
    /// redirect our writes and our sweep.
    #[cfg(unix)]
    #[test]
    fn a_symlinked_log_directory_is_refused() {
        let dir = tempfile::tempdir().unwrap();
        let elsewhere = dir.path().join("elsewhere");
        std::fs::create_dir(&elsewhere).unwrap();
        std::fs::create_dir(dir.path().join("Ceiling")).unwrap();
        std::os::unix::fs::symlink(&elsewhere, dir.path().join("Ceiling").join(LAUNCH_LOG_DIR))
            .unwrap();

        assert!(launch_log_dir_in(dir.path()).is_none());
    }

    #[test]
    fn the_log_directory_is_created_when_missing() {
        let dir = tempfile::tempdir().unwrap();

        let created = launch_log_dir_in(dir.path()).expect("log dir");

        assert!(created.is_dir());
        assert_eq!(created, dir.path().join("Ceiling").join(LAUNCH_LOG_DIR));
    }

    /// The owner and mode rule, checked on every platform. The unix branch that
    /// feeds it real `stat` values does not run on the Windows CI runner, so
    /// without this the rule itself would be untested there.
    #[test]
    fn a_log_directory_another_account_owns_is_refused() {
        assert_eq!(classify_log_dir(true, false, 0o700), LogDirVerdict::Refuse);
        assert_eq!(classify_log_dir(true, false, 0o755), LogDirVerdict::Refuse);
    }

    /// A symlink stats as "not a directory" here, because the caller uses
    /// `symlink_metadata`.
    #[test]
    fn a_log_directory_that_is_not_a_directory_is_refused() {
        assert_eq!(classify_log_dir(false, true, 0o700), LogDirVerdict::Refuse);
    }

    /// Ours but reachable by group or world: someone else could plant per-PID
    /// entries or read what we write, and we can fix it because we own it.
    #[test]
    fn a_log_directory_of_ours_that_others_can_reach_is_tightened() {
        for mode in [0o777, 0o755, 0o750, 0o701, 0o770] {
            assert_eq!(
                classify_log_dir(true, true, mode),
                LogDirVerdict::NeedsTightening,
                "mode {mode:o}"
            );
        }
    }

    #[test]
    fn a_private_log_directory_of_ours_is_usable() {
        assert_eq!(classify_log_dir(true, true, 0o700), LogDirVerdict::Usable);
        // Real `st_mode` carries the file-type bits as well.
        assert_eq!(classify_log_dir(true, true, 0o40700), LogDirVerdict::Usable);
    }

    #[cfg(unix)]
    #[test]
    fn a_new_log_directory_is_created_private() {
        use std::os::unix::fs::PermissionsExt;
        let dir = tempfile::tempdir().unwrap();

        let created = launch_log_dir_in(dir.path()).expect("log dir");

        let mode = std::fs::metadata(&created).unwrap().permissions().mode();
        assert_eq!(
            mode & 0o777,
            0o700,
            "no window where others can write in it"
        );
    }

    /// A world-writable directory we own is where the per-PID symlink gets
    /// planted, so it is tightened before we write anything into it.
    #[cfg(unix)]
    #[test]
    fn a_world_writable_log_directory_of_ours_is_tightened_before_use() {
        use std::os::unix::fs::PermissionsExt;
        let dir = tempfile::tempdir().unwrap();
        let planted = dir.path().join("Ceiling").join(LAUNCH_LOG_DIR);
        std::fs::create_dir_all(&planted).unwrap();
        std::fs::set_permissions(&planted, std::fs::Permissions::from_mode(0o777)).unwrap();

        let accepted = launch_log_dir_in(dir.path()).expect("our own directory is repairable");

        let mode = std::fs::metadata(&accepted).unwrap().permissions().mode();
        assert_eq!(mode & 0o777, 0o700);
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

    /// A cap on removals alone still walks every entry, which on a Windows
    /// `%TEMP%` holding tens of thousands of files is the cost worth avoiding.
    /// The walk itself has to stop.
    #[test]
    fn the_sweep_stops_at_the_entry_cap_not_only_the_removal_cap() {
        let dir = tempfile::tempdir().unwrap();
        for index in 0..12 {
            std::fs::write(
                dir.path().join(format!("codexbar_launch_{index}.log")),
                b"x",
            )
            .unwrap();
        }
        let later = SystemTime::now() + LAUNCH_LOG_MAX_AGE + Duration::from_secs(60);

        let outcome = sweep_launch_logs_in(dir.path(), None, later, 4, LAUNCH_LOG_SWEEP_LIMIT);

        assert_eq!(outcome.removed, 4, "four entries examined, four removed");
        assert!(!outcome.complete);
        assert_eq!(
            std::fs::read_dir(dir.path()).unwrap().count(),
            8,
            "the walk stopped instead of touching the other eight"
        );
    }

    #[test]
    fn the_sweep_stops_at_the_removal_cap() {
        let dir = tempfile::tempdir().unwrap();
        for index in 0..12 {
            std::fs::write(
                dir.path().join(format!("codexbar_launch_{index}.log")),
                b"x",
            )
            .unwrap();
        }
        let later = SystemTime::now() + LAUNCH_LOG_MAX_AGE + Duration::from_secs(60);

        let outcome = sweep_launch_logs_in(dir.path(), None, later, LAUNCH_LOG_SCAN_LIMIT, 3);

        assert_eq!(outcome.removed, 3);
        assert!(!outcome.complete);
    }

    /// Files the previous version wrote into the temp root are cleaned up, but
    /// the temp root is walked only until nothing is left there.
    #[test]
    fn the_legacy_temp_pass_clears_old_logs_then_retires() {
        let log_dir = tempfile::tempdir().unwrap();
        let temp_dir = tempfile::tempdir().unwrap();
        let legacy = temp_dir.path().join("codexbar_launch_7.log");
        let unrelated = temp_dir.path().join("some-installer.tmp");
        std::fs::write(&legacy, b"x").unwrap();
        std::fs::write(&unrelated, b"x").unwrap();
        let later = SystemTime::now() + LAUNCH_LOG_MAX_AGE + Duration::from_secs(60);

        sweep_legacy_temp_launch_logs(log_dir.path(), temp_dir.path(), later);

        assert!(!legacy.exists());
        assert!(unrelated.exists(), "unrelated temp files must survive");
        assert!(log_dir.path().join(LEGACY_SWEEP_MARKER).exists());

        // Second run: the marker is there, so the temp root is not walked at
        // all and this file is left where it is.
        let planted_after = temp_dir.path().join("codexbar_launch_8.log");
        std::fs::write(&planted_after, b"x").unwrap();
        sweep_legacy_temp_launch_logs(log_dir.path(), temp_dir.path(), later);

        assert!(
            planted_after.exists(),
            "steady-state runs must not read_dir the temp root"
        );
    }

    /// A legacy log still inside the age bound is not removed, so the pass is
    /// not finished and must run again rather than strand it.
    #[test]
    fn the_legacy_temp_pass_retries_while_logs_are_still_there() {
        let log_dir = tempfile::tempdir().unwrap();
        let temp_dir = tempfile::tempdir().unwrap();
        let recent = temp_dir.path().join("codexbar_launch_7.log");
        std::fs::write(&recent, b"x").unwrap();

        sweep_legacy_temp_launch_logs(log_dir.path(), temp_dir.path(), SystemTime::now());

        assert!(recent.exists(), "inside the 24-hour bound");
        assert!(!log_dir.path().join(LEGACY_SWEEP_MARKER).exists());
    }

    /// The marker sits in the launch-log directory, so the per-run sweep must
    /// not mistake it for a leftover and delete it.
    #[test]
    fn the_sweep_leaves_the_legacy_marker_alone() {
        let dir = tempfile::tempdir().unwrap();
        let marker = dir.path().join(LEGACY_SWEEP_MARKER);
        std::fs::write(&marker, b"").unwrap();
        let later = SystemTime::now() + LAUNCH_LOG_MAX_AGE + Duration::from_secs(60);

        sweep_stale_launch_logs_in(dir.path(), &dir.path().join("codexbar_launch_1.log"), later);

        assert!(marker.exists());
    }
}
