//! A driver that shells out to a program: **the** provider-agnostic escape
//! hatch.
//!
//! # Why a subprocess and not an HTTP client
//!
//! This crate takes no new dependencies (see `AGENTS.md`), and an HTTPS client
//! is a large one — TLS, a runtime, a JSON library — that every build of the
//! emulator would then carry, including the wasm and FFI ones, so that the
//! terminal frontend could talk to one vendor. Pushing the network out to a
//! small adapter program keeps the emulator dependency-free *and* makes the
//! model genuinely swappable: the adapter can be Anthropic's API, a local
//! model, the `claude` CLI, or a shell script that echoes canned JSON.
//!
//! The contract is deliberately tiny, so writing an adapter is an afternoon:
//!
//! ```text
//! stdin   one line of JSON: the state snapshot
//! stdout  one JSON object:  {"goal":…, "say":…, "plan":[…], "usage":{…}}
//! stderr  ignored by the panel, copied to the host's log
//! exit 0  success; anything else is a failed call
//! ```
//!
//! # It never blocks the emulator
//!
//! [`CommandDriver::submit`] spawns a worker **thread** which spawns the child;
//! the emulator thread only ever does a `try_recv`. A child that hangs is
//! killed by the host's timeout ([`crate::agent::governor::Budget::call_timeout`])
//! through [`Driver::cancel`], and the orphaned worker's result is discarded.
//!
//! # One process per decision
//!
//! Rather than a long-lived child speaking a stream protocol. It costs a few
//! milliseconds of spawn against a multi-second model call, and it buys three
//! things worth far more: a hung call is killed by killing a process rather
//! than by inventing a resynchronisation protocol, an adapter cannot leak state
//! between decisions, and one-shot CLIs (`claude -p`, `curl`, a shell script)
//! work as adapters unchanged.

use std::io::{Read, Write};
use std::process::{Child, Command, Stdio};
use std::sync::mpsc::{self, Receiver, TryRecvError};
use std::sync::{Arc, Mutex};

use super::driver::{Driver, DriverError, Poll};
use super::json::MAX_INPUT;

/// Most stderr kept from one call, for the log. A misbehaving adapter can
/// produce a lot.
const MAX_STDERR: usize = 8 * 1024;

pub struct CommandDriver {
    name: String,
    program: String,
    args: Vec<String>,
    /// Set while a call is in flight.
    rx: Option<Receiver<Result<Reply, DriverError>>>,
    /// The running child, so the host's timeout can kill it.
    child: Arc<Mutex<Option<Child>>>,
    /// Anything the last call wrote to stderr, for the log file **only**.
    last_stderr: String,
}

struct Reply {
    stdout: String,
    stderr: String,
}

impl CommandDriver {
    /// `program` plus fixed arguments. The state snapshot is written to the
    /// child's stdin, not passed as an argument — a command line is visible in
    /// `ps` to every user on the machine, and the snapshot is large.
    pub fn new(program: &str, args: Vec<String>) -> CommandDriver {
        CommandDriver {
            name: short_name(program),
            program: program.to_string(),
            args,
            rx: None,
            child: Arc::new(Mutex::new(None)),
            last_stderr: String::new(),
        }
    }

    /// Override the name shown in the status panel.
    pub fn with_name(mut self, name: &str) -> Self {
        self.name = name.to_string();
        self
    }

    fn kill_child(&self) {
        if let Ok(mut guard) = self.child.lock() {
            if let Some(mut c) = guard.take() {
                let _ = c.kill();
                let _ = c.wait();
            }
        }
    }
}

/// The last path component, so the panel says `claude_adapter.py` and not a
/// whole absolute path. Paths are internal detail.
fn short_name(program: &str) -> String {
    program
        .rsplit(['/', '\\'])
        .next()
        .unwrap_or(program)
        .to_string()
}

impl Driver for CommandDriver {
    fn name(&self) -> &str {
        &self.name
    }

    fn costs_money(&self) -> bool {
        true
    }

    fn submit(&mut self, request: &str) -> bool {
        if self.rx.is_some() {
            return false;
        }
        let (tx, rx) = mpsc::channel();
        let program = self.program.clone();
        let args = self.args.clone();
        let body = format!("{request}\n");
        let slot = Arc::clone(&self.child);

        let spawned = std::thread::Builder::new()
            .name("gb-agent-driver".to_string())
            .spawn(move || {
                let child = Command::new(&program)
                    .args(&args)
                    .stdin(Stdio::piped())
                    .stdout(Stdio::piped())
                    .stderr(Stdio::piped())
                    .spawn();
                let mut child = match child {
                    Ok(c) => c,
                    Err(e) => {
                        let _ = tx.send(Err(DriverError::Crashed(format!(
                            "could not run the agent command: {e}"
                        ))));
                        return;
                    }
                };
                let mut stdin = child.stdin.take();
                let mut stdout = child.stdout.take();
                let mut stderr = child.stderr.take();
                if let Ok(mut g) = slot.lock() {
                    *g = Some(child);
                }
                if let Some(si) = stdin.as_mut() {
                    // A child that never reads its stdin would block us here;
                    // the write is on this worker thread, so the emulator does
                    // not care, and the host's timeout kills the child.
                    let _ = si.write_all(body.as_bytes());
                }
                drop(stdin);

                let mut out = String::new();
                if let Some(so) = stdout.as_mut() {
                    let mut buf = Vec::new();
                    let _ = so.take(MAX_INPUT as u64).read_to_end(&mut buf);
                    out = String::from_utf8_lossy(&buf).into_owned();
                }
                let mut err = String::new();
                if let Some(se) = stderr.as_mut() {
                    let mut buf = Vec::new();
                    let _ = se.take(MAX_STDERR as u64).read_to_end(&mut buf);
                    err = String::from_utf8_lossy(&buf).into_owned();
                }

                let status = {
                    let taken = slot.lock().ok().and_then(|mut g| g.take());
                    match taken {
                        Some(mut c) => c.wait().ok(),
                        // Cancelled: the host already killed and reaped it.
                        None => None,
                    }
                };
                let result = match status {
                    Some(s) if s.success() => Ok(Reply {
                        stdout: out,
                        stderr: err,
                    }),
                    Some(s) => Err(DriverError::Crashed(format!(
                        "agent command exited with {s}: {}",
                        err.trim()
                    ))),
                    None => Err(DriverError::Crashed(
                        "agent command was cancelled".to_string(),
                    )),
                };
                let _ = tx.send(result);
            });

        match spawned {
            Ok(_) => {
                self.rx = Some(rx);
                true
            }
            Err(_) => false,
        }
    }

    fn poll(&mut self) -> Poll {
        let Some(rx) = self.rx.as_ref() else {
            return Poll::Idle;
        };
        match rx.try_recv() {
            Err(TryRecvError::Empty) => Poll::Pending,
            Err(TryRecvError::Disconnected) => {
                self.rx = None;
                Poll::Failed(DriverError::Crashed(
                    "the agent command's worker vanished".to_string(),
                ))
            }
            Ok(Ok(reply)) => {
                self.rx = None;
                self.last_stderr = reply.stderr;
                if reply.stdout.trim().is_empty() {
                    return Poll::Failed(DriverError::Unreadable(
                        "the agent command printed nothing".to_string(),
                    ));
                }
                Poll::Ready(reply.stdout)
            }
            Ok(Err(e)) => {
                self.rx = None;
                Poll::Failed(e)
            }
        }
    }

    fn cancel(&mut self) {
        self.kill_child();
        self.rx = None;
    }

    /// Whatever the last call wrote to stderr. For the host's log file — it
    /// must never reach the commentary panel.
    fn take_stderr(&mut self) -> String {
        std::mem::take(&mut self.last_stderr)
    }
}

impl Drop for CommandDriver {
    /// A killed child is the *last* resort, not the mechanism — the host's
    /// timeout cancels first. This only catches the emulator exiting mid-call.
    fn drop(&mut self) {
        self.kill_child();
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Poll until ready, with a bound so a broken test fails rather than hangs.
    fn wait(d: &mut CommandDriver) -> Poll {
        for _ in 0..2000 {
            match d.poll() {
                Poll::Pending => std::thread::sleep(std::time::Duration::from_millis(5)),
                other => return other,
            }
        }
        panic!("driver never answered");
    }

    #[test]
    fn a_command_receives_the_state_on_stdin_and_answers_on_stdout() {
        // `cat` is the smallest possible adapter: it echoes the request back,
        // which is valid JSON, so this exercises the whole path.
        let mut d = CommandDriver::new("cat", vec![]);
        assert!(d.submit(r#"{"hello":"world"}"#));
        match wait(&mut d) {
            Poll::Ready(s) => assert!(s.contains("\"hello\""), "{s}"),
            other => panic!("{other:?}"),
        }
        assert_eq!(d.poll(), Poll::Idle);
    }

    #[test]
    fn a_command_that_does_not_exist_is_a_failed_call_not_a_panic() {
        let mut d = CommandDriver::new("gb-no-such-program-exists", vec![]);
        assert!(d.submit("{}"));
        match wait(&mut d) {
            Poll::Failed(DriverError::Crashed(_)) => {}
            other => panic!("{other:?}"),
        }
    }

    #[test]
    fn a_non_zero_exit_is_a_failed_call_and_its_stderr_stays_out_of_the_reply() {
        let mut d = CommandDriver::new(
            "sh",
            vec!["-c".into(), "echo 'secret sk-ant-oops' >&2; exit 3".into()],
        );
        assert!(d.submit("{}"));
        match wait(&mut d) {
            Poll::Failed(e) => {
                // The detail exists for the log...
                assert!(e.detail().contains("exited"), "{}", e.detail());
                // ...and the panel only ever sees the category.
                assert_eq!(e.category(), crate::agent::commentary::Problem::Crashed);
            }
            other => panic!("{other:?}"),
        }
    }

    #[test]
    fn an_empty_answer_is_unreadable_rather_than_an_empty_plan() {
        let mut d = CommandDriver::new("true", vec![]);
        assert!(d.submit("{}"));
        match wait(&mut d) {
            Poll::Failed(DriverError::Unreadable(_)) => {}
            other => panic!("{other:?}"),
        }
    }

    /// The property the whole design rests on: submitting is instant even when
    /// the command is slow, and polling never waits.
    #[test]
    fn submit_and_poll_return_immediately_even_for_a_slow_command() {
        let mut d =
            CommandDriver::new("sh", vec!["-c".into(), "sleep 5; echo {}".into()]);
        let t = std::time::Instant::now();
        assert!(d.submit("{}"));
        for _ in 0..50 {
            assert_eq!(d.poll(), Poll::Pending);
        }
        assert!(
            t.elapsed() < std::time::Duration::from_millis(500),
            "submit+poll took {:?} — the emulator would have stalled",
            t.elapsed()
        );
        // Cancelling kills it rather than waiting the remaining seconds out.
        d.cancel();
        assert!(t.elapsed() < std::time::Duration::from_secs(2));
        assert_eq!(d.poll(), Poll::Idle);
    }

    #[test]
    fn a_second_submit_while_busy_is_refused() {
        let mut d = CommandDriver::new("sh", vec!["-c".into(), "sleep 2".into()]);
        assert!(d.submit("{}"));
        assert!(!d.submit("{}"), "one call at a time");
        d.cancel();
    }

    #[test]
    fn the_displayed_name_is_the_command_not_its_path() {
        let d = CommandDriver::new("/opt/secret/place/my_adapter.py", vec![]);
        assert_eq!(d.name(), "my_adapter.py");
    }
}
