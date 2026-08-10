//! FFmpeg process execution, diagnostics, and cancellation.

use std::{io, path::PathBuf, process::Stdio, time::Instant};

use async_trait::async_trait;
use command_group::{AsyncCommandGroup as _, AsyncGroupChild};
use sonicmux_backend::{
    BackendCapabilities, BackendError, BackendExecution, BackendReport, CapabilityRequest,
    MediaBackend, ProgressEvent,
};
use sonicmux_core::MediaInfo;
use thiserror::Error;
use tokio::{
    io::{AsyncRead, AsyncReadExt as _},
    process::Command,
    sync::mpsc,
    task::{JoinError, JoinHandle},
    time::{Duration, sleep},
};
use tokio_util::sync::CancellationToken;

use crate::{
    FfmpegCliBackend,
    command::{ArgumentError, build_execution_arguments},
    progress::{ProgressError, ProgressReadReport, read_progress},
};

const MAX_STDERR_BYTES: usize = 256 * 1024;
const PROCESS_POLL_INTERVAL: Duration = Duration::from_millis(25);

/// Error produced while invoking FFmpeg.
#[derive(Debug, Error)]
#[non_exhaustive]
pub enum ExecutionError {
    /// Typed argument generation failed.
    #[error(transparent)]
    Arguments(#[from] ArgumentError),
    /// FFmpeg could not be launched.
    #[error("failed to launch FFmpeg at {executable}: {source}")]
    Spawn {
        /// Configured executable path.
        executable: PathBuf,
        /// Operating-system error.
        #[source]
        source: io::Error,
    },
    /// A configured child pipe was unexpectedly unavailable.
    #[error("FFmpeg {pipe} pipe was unavailable")]
    MissingPipe {
        /// Pipe name used in diagnostics.
        pipe: &'static str,
    },
    /// Progress protocol processing failed.
    #[error(transparent)]
    Progress(#[from] ProgressError),
    /// Waiting for FFmpeg failed.
    #[error("failed to wait for FFmpeg: {source}")]
    Wait {
        /// Operating-system error.
        #[source]
        source: io::Error,
    },
    /// Terminating the process group failed after it was explicitly reaped.
    #[error("failed to terminate FFmpeg process group: {source}")]
    Terminate {
        /// Operating-system error.
        #[source]
        source: io::Error,
    },
    /// An output reader task failed unexpectedly.
    #[error("FFmpeg {pipe} reader task failed: {message}")]
    ReaderTask {
        /// Pipe name.
        pipe: &'static str,
        /// Bounded join diagnostic.
        message: String,
    },
    /// FFmpeg exited unsuccessfully.
    #[error("FFmpeg exited with code {code:?}: {stderr}")]
    Failed {
        /// Process exit code when supplied by the platform.
        code: Option<i32>,
        /// Bounded standard-error tail.
        stderr: String,
    },
    /// A successful exit omitted the required terminal progress record.
    #[error("FFmpeg exited successfully without `progress=end`")]
    MissingProgressEnd,
    /// Cancellation completed after the process group was reaped.
    #[error("FFmpeg execution cancelled")]
    Cancelled,
}

impl FfmpegCliBackend {
    /// Executes one typed request with adapter-specific errors.
    ///
    /// # Errors
    ///
    /// Returns [`ExecutionError`] for argument, process, progress, exit, or
    /// cancellation failures.
    pub async fn execute_typed(
        &self,
        request: BackendExecution,
        progress: mpsc::Sender<ProgressEvent>,
        cancel: CancellationToken,
    ) -> Result<BackendReport, ExecutionError> {
        let built = build_execution_arguments(request.plan(), request.staging_path())?;
        let (arguments, warnings) = built.into_parts();
        let mut command = Command::new(self.ffmpeg_path());
        command
            .args(arguments)
            .stdin(Stdio::null())
            .stdout(Stdio::piped())
            .stderr(Stdio::piped());
        let mut group = command.group();
        group.kill_on_drop(true);
        #[cfg(windows)]
        group.creation_flags(0x0800_0000);
        let mut child = group.spawn().map_err(|source| ExecutionError::Spawn {
            executable: self.ffmpeg_path().to_path_buf(),
            source,
        })?;
        let stdout = match child.inner().stdout.take() {
            Some(stdout) => stdout,
            None => {
                terminate_and_reap(&mut child).await?;
                return Err(ExecutionError::MissingPipe { pipe: "stdout" });
            }
        };
        let stderr = match child.inner().stderr.take() {
            Some(stderr) => stderr,
            None => {
                terminate_and_reap(&mut child).await?;
                return Err(ExecutionError::MissingPipe { pipe: "stderr" });
            }
        };
        let started = Instant::now();
        let _dropped = progress.try_send(ProgressEvent::Started);
        let mut progress_task = Some(tokio::spawn(read_progress(stdout, progress)));
        let stderr_task = tokio::spawn(read_tail(stderr, MAX_STDERR_BYTES));
        let mut progress_report = None;

        let status = loop {
            if progress_task.as_ref().is_some_and(JoinHandle::is_finished) {
                let task = progress_task
                    .take()
                    .ok_or_else(|| ExecutionError::ReaderTask {
                        pipe: "stdout",
                        message: "progress reader handle disappeared".to_owned(),
                    })?;
                match join_progress(task.await) {
                    Ok(report) => progress_report = Some(report),
                    Err(error) => {
                        terminate_and_reap(&mut child).await?;
                        join_after_termination(None, stderr_task).await;
                        return Err(error);
                    }
                }
            }

            if let Some(status) = child
                .try_wait()
                .map_err(|source| ExecutionError::Wait { source })?
            {
                break status;
            }

            tokio::select! {
                () = cancel.cancelled() => {
                    terminate_and_reap(&mut child).await?;
                    join_after_termination(progress_task, stderr_task).await;
                    return Err(ExecutionError::Cancelled);
                }
                () = sleep(PROCESS_POLL_INTERVAL) => {}
            }
        };

        let progress_report = match progress_report {
            Some(report) => report,
            None => {
                let task = progress_task.ok_or_else(|| ExecutionError::ReaderTask {
                    pipe: "stdout",
                    message: "progress reader handle disappeared".to_owned(),
                })?;
                join_progress(task.await)?
            }
        };
        let stderr = join_stderr(stderr_task.await)?;
        if !status.success() {
            return Err(ExecutionError::Failed {
                code: status.code(),
                stderr: String::from_utf8_lossy(&stderr).trim().to_owned(),
            });
        }
        if !progress_report.saw_end {
            return Err(ExecutionError::MissingProgressEnd);
        }
        Ok(BackendReport::new(
            started.elapsed(),
            progress_report.last,
            warnings,
        ))
    }
}

async fn terminate_and_reap(child: &mut AsyncGroupChild) -> Result<(), ExecutionError> {
    if child
        .try_wait()
        .map_err(|source| ExecutionError::Wait { source })?
        .is_some()
    {
        return Ok(());
    }
    let kill_error = child.start_kill().err();
    let wait_result = child.wait().await;
    if let Err(source) = wait_result {
        return Err(ExecutionError::Wait { source });
    }
    if let Some(source) = kill_error {
        return Err(ExecutionError::Terminate { source });
    }
    Ok(())
}

async fn join_after_termination(
    progress_task: Option<JoinHandle<Result<ProgressReadReport, ProgressError>>>,
    stderr_task: JoinHandle<Result<Vec<u8>, io::Error>>,
) {
    if let Some(task) = progress_task {
        let _ignored = task.await;
    }
    let _ignored = stderr_task.await;
}

fn join_progress(
    result: Result<Result<ProgressReadReport, ProgressError>, JoinError>,
) -> Result<ProgressReadReport, ExecutionError> {
    result
        .map_err(|error| ExecutionError::ReaderTask {
            pipe: "stdout",
            message: error.to_string(),
        })?
        .map_err(ExecutionError::from)
}

fn join_stderr(
    result: Result<Result<Vec<u8>, io::Error>, JoinError>,
) -> Result<Vec<u8>, ExecutionError> {
    result
        .map_err(|error| ExecutionError::ReaderTask {
            pipe: "stderr",
            message: error.to_string(),
        })?
        .map_err(|error| ExecutionError::ReaderTask {
            pipe: "stderr",
            message: error.to_string(),
        })
}

async fn read_tail<R>(mut reader: R, limit: usize) -> Result<Vec<u8>, io::Error>
where
    R: AsyncRead + Unpin,
{
    let mut tail = Vec::new();
    let mut chunk = [0_u8; 4096];
    loop {
        let read = reader.read(&mut chunk).await?;
        if read == 0 {
            break;
        }
        tail.extend_from_slice(&chunk[..read]);
        if tail.len() > limit {
            let excess = tail.len() - limit;
            tail.drain(..excess);
        }
    }
    Ok(tail)
}

#[async_trait]
impl MediaBackend for FfmpegCliBackend {
    async fn probe(
        &self,
        path: &std::path::Path,
        cancel: CancellationToken,
    ) -> Result<MediaInfo, BackendError> {
        self.probe_with_cancel(path, cancel)
            .await
            .map_err(|source| match source {
                crate::ProbeError::Cancelled => BackendError::Cancelled,
                source => BackendError::Probe {
                    path: path.to_path_buf(),
                    source: Box::new(source),
                },
            })
    }

    async fn execute(
        &self,
        request: BackendExecution,
        progress: mpsc::Sender<ProgressEvent>,
        cancel: CancellationToken,
    ) -> Result<BackendReport, BackendError> {
        self.execute_typed(request, progress, cancel)
            .await
            .map_err(|source| match source {
                ExecutionError::Cancelled => BackendError::Cancelled,
                source => BackendError::Execute {
                    source: Box::new(source),
                },
            })
    }

    async fn capabilities(
        &self,
        request: CapabilityRequest,
        cancel: CancellationToken,
    ) -> Result<BackendCapabilities, BackendError> {
        self.inspect_capabilities(request, cancel).await
    }
}
