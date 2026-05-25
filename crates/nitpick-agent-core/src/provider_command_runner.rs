use std::{
    io::{Read, Write},
    process::{Command, ExitStatus, Stdio},
    sync::mpsc::{self, Receiver, Sender},
    thread::JoinHandle,
    time::{Duration, Instant},
};

use crate::{AgentError, AgentResult, ProviderRunSink};

pub struct ProviderCommandOutput {
    pub status: ExitStatus,
    pub stdout: Vec<u8>,
    pub stderr: Vec<u8>,
    pub duration_ms: u128,
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
    ) -> AgentResult<ProviderCommandOutput> {
        let started = Instant::now();
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

        let mut output = collect_provider_output(child, stream_rx, run_sink)?;
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
) -> AgentResult<ProviderCommandOutput> {
    let mut stdout = Vec::new();
    let mut stderr = Vec::new();
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
        match rx.recv_timeout(Duration::from_millis(50)) {
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
    })
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
