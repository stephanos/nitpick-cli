use std::{
    io::{Read, Write},
    process::{Command, ExitStatus, Stdio},
    sync::mpsc::{self, Receiver, Sender},
    thread::JoinHandle,
    time::{Duration, Instant},
};

#[cfg(unix)]
use std::os::unix::process::CommandExt;

use crate::{AgentError, AgentResult, ProviderRunSink};

#[cfg(unix)]
const SIGKILL: i32 = 9;

#[cfg(unix)]
unsafe extern "C" {
    fn kill(pid: i32, sig: i32) -> i32;
    fn setpgid(pid: i32, pgid: i32) -> i32;
}

pub struct ProviderCommandOutput {
    pub status: ExitStatus,
    pub stdout: Vec<u8>,
    pub stderr: Vec<u8>,
    pub duration_ms: u128,
    pub timed_out: bool,
    pub cancelled: bool,
}

pub struct ProviderCommandRunner<'a> {
    provider: &'a str,
    command_display: &'a str,
}

impl<'a> ProviderCommandRunner<'a> {
    pub fn new(provider: &'a str, command_display: &'a str) -> Self {
        Self {
            provider,
            command_display,
        }
    }

    pub fn run(
        &self,
        mut command: Command,
        prompt: &str,
        run_sink: &dyn ProviderRunSink,
        timeout: Option<Duration>,
    ) -> AgentResult<ProviderCommandOutput> {
        let started = Instant::now();
        configure_provider_command(&mut command);
        let mut child = command
            .stdin(Stdio::piped())
            .stdout(Stdio::piped())
            .stderr(Stdio::piped())
            .spawn()
            .map_err(|error| {
                AgentError::provider(format!(
                    "failed to start {} provider command `{}`: {error}",
                    self.provider, self.command_display
                ))
            })?;

        let stdout = child
            .stdout
            .take()
            .ok_or_else(|| AgentError::provider("provider command stdout unavailable"))?;
        let stderr = child
            .stderr
            .take()
            .ok_or_else(|| AgentError::provider("provider command stderr unavailable"))?;
        let (stream_tx, stream_rx) = mpsc::channel();
        let stdout_reader =
            spawn_provider_stream_reader(ProviderStream::Stdout, stdout, stream_tx.clone());
        let stderr_reader = spawn_provider_stream_reader(ProviderStream::Stderr, stderr, stream_tx);

        child
            .stdin
            .take()
            .ok_or_else(|| AgentError::provider("provider command stdin unavailable"))?
            .write_all(prompt.as_bytes())
            .map_err(|error| AgentError::provider(format!("write provider prompt: {error}")))?;

        let mut output = collect_provider_output(child, stream_rx, run_sink, timeout)?;
        join_provider_stream_reader(stdout_reader)?;
        join_provider_stream_reader(stderr_reader)?;
        output.duration_ms = started.elapsed().as_millis();
        Ok(output)
    }
}

#[derive(Clone, Copy)]
enum ProviderStream {
    Stdout,
    Stderr,
}

struct ProviderStreamChunk {
    stream: ProviderStream,
    bytes: Vec<u8>,
}

fn spawn_provider_stream_reader<R>(
    stream: ProviderStream,
    mut reader: R,
    tx: Sender<AgentResult<ProviderStreamChunk>>,
) -> JoinHandle<()>
where
    R: Read + Send + 'static,
{
    std::thread::spawn(move || {
        let mut buffer = [0; 4096];
        loop {
            match reader.read(&mut buffer) {
                Ok(0) => break,
                Ok(count) => {
                    let chunk = ProviderStreamChunk {
                        stream,
                        bytes: buffer[..count].to_vec(),
                    };
                    if tx.send(Ok(chunk)).is_err() {
                        break;
                    }
                }
                Err(error) => {
                    let _ = tx.send(Err(AgentError::provider(format!(
                        "read provider output: {error}"
                    ))));
                    break;
                }
            }
        }
    })
}

fn collect_provider_output(
    mut child: std::process::Child,
    rx: Receiver<AgentResult<ProviderStreamChunk>>,
    run_sink: &dyn ProviderRunSink,
    timeout: Option<Duration>,
) -> AgentResult<ProviderCommandOutput> {
    let mut stdout = Vec::new();
    let mut stderr = Vec::new();
    let deadline = timeout.map(|timeout| Instant::now() + timeout);
    let mut timed_out = false;
    let mut cancelled = false;
    let status = loop {
        while let Ok(chunk) = rx.try_recv() {
            append_provider_stream_chunk(chunk?, &mut stdout, &mut stderr, run_sink)?;
        }
        if let Some(status) = child
            .try_wait()
            .map_err(|error| AgentError::provider(format!("wait for provider command: {error}")))?
        {
            break status;
        }
        if deadline.is_some_and(|deadline| Instant::now() >= deadline) {
            timed_out = true;
            kill_provider_child(&mut child, "timed out")?;
            break child.wait().map_err(|error| {
                AgentError::provider(format!("wait for timed out provider command: {error}"))
            })?;
        }
        if run_sink.is_cancelled()? {
            cancelled = true;
            kill_provider_child(&mut child, "cancelled")?;
            break child.wait().map_err(|error| {
                AgentError::provider(format!("wait for cancelled provider command: {error}"))
            })?;
        }
        let wait = deadline
            .map(|deadline| deadline.saturating_duration_since(Instant::now()))
            .unwrap_or(Duration::from_millis(50))
            .min(Duration::from_millis(50));
        match rx.recv_timeout(wait) {
            Ok(chunk) => append_provider_stream_chunk(chunk?, &mut stdout, &mut stderr, run_sink)?,
            Err(mpsc::RecvTimeoutError::Timeout) => {}
            Err(mpsc::RecvTimeoutError::Disconnected) => {}
        }
    };
    loop {
        match rx.recv() {
            Ok(chunk) => append_provider_stream_chunk(chunk?, &mut stdout, &mut stderr, run_sink)?,
            Err(_) => break,
        }
    }
    run_sink.flush()?;
    Ok(ProviderCommandOutput {
        status,
        stdout,
        stderr,
        duration_ms: 0,
        timed_out,
        cancelled,
    })
}

fn configure_provider_command(command: &mut Command) {
    #[cfg(unix)]
    // Put the provider in its own process group so cancellation and timeout can
    // terminate helper processes that inherit stdout/stderr pipes.
    unsafe {
        command.pre_exec(|| {
            if setpgid(0, 0) == 0 {
                Ok(())
            } else {
                Err(std::io::Error::last_os_error())
            }
        });
    }
}

fn kill_provider_child(child: &mut std::process::Child, reason: &str) -> AgentResult<()> {
    #[cfg(unix)]
    {
        let process_group = -(child.id() as i32);
        // SAFETY: kill is called with a process-group id created for this child
        // in configure_provider_command. On failure, fall back to Child::kill.
        if unsafe { kill(process_group, SIGKILL) } == 0 {
            return Ok(());
        }
    }
    child
        .kill()
        .map_err(|error| AgentError::provider(format!("kill {reason} provider command: {error}")))
}

fn append_provider_stream_chunk(
    chunk: ProviderStreamChunk,
    stdout: &mut Vec<u8>,
    stderr: &mut Vec<u8>,
    run_sink: &dyn ProviderRunSink,
) -> AgentResult<()> {
    match chunk.stream {
        ProviderStream::Stdout => {
            stdout.extend_from_slice(&chunk.bytes);
            run_sink.append_stdout(&chunk.bytes)?;
        }
        ProviderStream::Stderr => {
            stderr.extend_from_slice(&chunk.bytes);
            run_sink.append_stderr(&chunk.bytes)?;
        }
    }
    Ok(())
}

fn join_provider_stream_reader(handle: JoinHandle<()>) -> AgentResult<()> {
    handle
        .join()
        .map_err(|_| AgentError::provider("provider output reader thread panicked"))
}
