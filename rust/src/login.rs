//! Login flow runners for various providers
//!
//! Runs CLI login commands and captures output/URLs

#![allow(dead_code)]

use regex_lite::Regex;
use std::io::{BufRead, BufReader, Read};
#[cfg(windows)]
use std::os::windows::process::CommandExt;
use std::process::{Child, Command, Stdio};
use std::sync::mpsc;
use std::thread;
use std::time::{Duration, Instant};

const CLAUDE_LOGIN_ARGS: &[&str] = &["auth", "login"];
const CODEX_LOGIN_ARGS: &[&str] = &["login", "--device-auth"];

/// Result of a login attempt
#[derive(Debug, Clone)]
pub struct LoginResult {
    pub outcome: LoginOutcome,
    pub output: String,
    pub auth_link: Option<String>,
}

/// Outcome of login attempt
#[derive(Debug, Clone)]
pub enum LoginOutcome {
    Success,
    TimedOut,
    Failed { status: i32 },
    MissingBinary,
    LaunchFailed(String),
}

/// Phase of the login process
#[derive(Debug, Clone, Copy, PartialEq)]
pub enum LoginPhase {
    Idle,
    Requesting,
    WaitingBrowser,
    Complete,
}

/// Run Claude CLI login
pub fn run_claude_login<F>(timeout_secs: u64, on_phase: F) -> LoginResult
where
    F: Fn(LoginPhase) + Send + 'static,
{
    run_cli_login(
        "claude",
        CLAUDE_LOGIN_ARGS,
        timeout_secs,
        on_phase,
        &[
            "Successfully logged in",
            "Login successful",
            "Logged in successfully",
        ],
    )
}

/// Run Codex CLI login
pub fn run_codex_login<F>(timeout_secs: u64, on_phase: F) -> LoginResult
where
    F: Fn(LoginPhase) + Send + 'static,
{
    run_cli_login(
        "codex",
        CODEX_LOGIN_ARGS,
        timeout_secs,
        on_phase,
        &[
            "Successfully logged in",
            "Login successful",
            "Logged in successfully",
        ],
    )
}

/// Run Copilot/GitHub device flow login
pub fn run_copilot_login<F>(timeout_secs: u64, on_phase: F) -> LoginResult
where
    F: Fn(LoginPhase) + Send + 'static,
{
    run_cli_login(
        "gh",
        &["auth", "login", "-w"],
        timeout_secs,
        on_phase,
        &["Logged in as", "Authentication complete"],
    )
}

/// Generic CLI login runner
fn run_cli_login<F>(
    binary: &str,
    args: &[&str],
    timeout_secs: u64,
    on_phase: F,
    success_markers: &[&str],
) -> LoginResult
where
    F: Fn(LoginPhase) + Send + 'static,
{
    let binary_path = match which::which(binary) {
        Ok(p) => p,
        Err(_) => return missing_binary_result(binary),
    };

    on_phase(LoginPhase::Requesting);

    let child = match spawn_login_process(binary_path.as_path(), args) {
        Ok(c) => c,
        Err(e) => return launch_failed_result(e),
    };

    let state = CliLoginState::new(timeout_secs, &on_phase, success_markers);

    monitor_login_process(child, state, &on_phase)
}

fn missing_binary_result(binary: &str) -> LoginResult {
    LoginResult {
        outcome: LoginOutcome::MissingBinary,
        output: format!("{} not found in PATH", binary),
        auth_link: None,
    }
}

fn launch_failed_result(error: String) -> LoginResult {
    LoginResult {
        outcome: LoginOutcome::LaunchFailed(error),
        output: String::new(),
        auth_link: None,
    }
}

fn spawn_login_process(binary_path: &std::path::Path, args: &[&str]) -> Result<Child, String> {
    #[cfg(windows)]
    const CREATE_NO_WINDOW: u32 = 0x08000000;

    let mut cmd = Command::new(binary_path);
    cmd.args(args).stdout(Stdio::piped()).stderr(Stdio::piped());
    #[cfg(windows)]
    cmd.creation_flags(CREATE_NO_WINDOW);

    cmd.spawn().map_err(|e| e.to_string())
}

struct CliLoginState<'a, F>
where
    F: Fn(LoginPhase),
{
    output: String,
    auth_link: Option<String>,
    url_regex: Regex,
    on_phase: &'a F,
    success_markers: &'a [&'a str],
    start: Instant,
    timeout: Duration,
}

impl<'a, F> CliLoginState<'a, F>
where
    F: Fn(LoginPhase),
{
    fn new(timeout_secs: u64, on_phase: &'a F, success_markers: &'a [&'a str]) -> Self {
        Self {
            output: String::new(),
            auth_link: None,
            url_regex: Regex::new(r"https?://[A-Za-z0-9._~:/?#\[\]@!$&'()*+,;=%-]+").unwrap(),
            on_phase,
            success_markers,
            start: Instant::now(),
            timeout: Duration::from_secs(timeout_secs),
        }
    }

    fn handle_line(&mut self, line: &str) -> Option<LoginOutcome> {
        self.output.push_str(line);
        self.output.push('\n');
        self.capture_auth_link(line);

        if self
            .success_markers
            .iter()
            .any(|marker| line.contains(marker))
        {
            (self.on_phase)(LoginPhase::Complete);
            return Some(LoginOutcome::Success);
        }

        self.start
            .elapsed()
            .gt(&self.timeout)
            .then_some(LoginOutcome::TimedOut)
    }

    fn capture_auth_link(&mut self, line: &str) {
        if self.auth_link.is_some() {
            return;
        }

        let Some(m) = self.url_regex.find(line) else {
            return;
        };

        self.auth_link = Some(m.as_str().to_string());
        (self.on_phase)(LoginPhase::WaitingBrowser);
        let _ = open::that(m.as_str());
    }

    fn into_result(self, outcome: LoginOutcome) -> LoginResult {
        LoginResult {
            outcome,
            output: self.output,
            auth_link: self.auth_link,
        }
    }
}

fn forward_login_stream<R>(stream: Option<R>, sender: mpsc::Sender<String>)
where
    R: Read + Send + 'static,
{
    let Some(stream) = stream else {
        return;
    };
    thread::spawn(move || {
        for line in BufReader::new(stream).lines().map_while(Result::ok) {
            if sender.send(line).is_err() {
                break;
            }
        }
    });
}

fn monitor_login_process<F>(
    mut child: Child,
    mut state: CliLoginState<'_, F>,
    on_phase: &F,
) -> LoginResult
where
    F: Fn(LoginPhase),
{
    const POLL_INTERVAL: Duration = Duration::from_millis(100);

    let (sender, receiver) = mpsc::channel();
    forward_login_stream(child.stdout.take(), sender.clone());
    forward_login_stream(child.stderr.take(), sender);

    loop {
        if state.start.elapsed() >= state.timeout {
            return stop_child_with_outcome(&mut child, state, LoginOutcome::TimedOut);
        }

        match receiver.recv_timeout(POLL_INTERVAL) {
            Ok(line) => {
                if let Some(outcome) = state.handle_line(&line) {
                    return stop_child_with_outcome(&mut child, state, outcome);
                }
            }
            Err(mpsc::RecvTimeoutError::Timeout) => {}
            Err(mpsc::RecvTimeoutError::Disconnected) => {}
        }

        match child.try_wait() {
            Ok(Some(status)) => {
                for line in receiver.try_iter() {
                    if let Some(outcome) = state.handle_line(&line) {
                        return state.into_result(outcome);
                    }
                }
                if status.success() {
                    on_phase(LoginPhase::Complete);
                    return state.into_result(LoginOutcome::Success);
                }
                return state.into_result(LoginOutcome::Failed {
                    status: status.code().unwrap_or(-1),
                });
            }
            Ok(None) => {}
            Err(error) => {
                return stop_child_with_outcome(
                    &mut child,
                    state,
                    LoginOutcome::LaunchFailed(error.to_string()),
                );
            }
        }
    }
}

fn stop_child_with_outcome<F>(
    child: &mut Child,
    state: CliLoginState<'_, F>,
    outcome: LoginOutcome,
) -> LoginResult
where
    F: Fn(LoginPhase),
{
    let _ = child.kill();
    let _ = child.wait();
    state.into_result(outcome)
}

/// Open a URL in the default browser
pub fn open_auth_url(url: &str) -> anyhow::Result<()> {
    open::that(url)?;
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn login_commands_match_current_dedicated_auth_surfaces() {
        assert_eq!(CLAUDE_LOGIN_ARGS, ["auth", "login"]);
        assert_eq!(CODEX_LOGIN_ARGS, ["login", "--device-auth"]);
    }

    #[test]
    fn success_marker_completes_and_notifies_the_caller() {
        let phases = std::cell::RefCell::new(Vec::new());
        let on_phase = |phase| phases.borrow_mut().push(phase);
        let mut state = CliLoginState::new(30, &on_phase, &["Login successful"]);

        let outcome = state.handle_line("Login successful");

        assert!(matches!(outcome, Some(LoginOutcome::Success)));
        assert_eq!(*phases.borrow(), vec![LoginPhase::Complete]);
    }
}
