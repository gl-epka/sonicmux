//! Bounded FFmpeg capability inspection for `sonicmux doctor`.

use std::{io, path::Path, process::Stdio};

use command_group::{AsyncCommandGroup as _, AsyncGroupChild};
use sonicmux_backend::{
    BackendCapabilities, BackendError, BackendToolInfo, BackendToolRole, CapabilityCheck,
    CapabilityRequest, MediaCapability,
};
use tokio::{io::AsyncReadExt as _, process::Command};
use tokio_util::sync::CancellationToken;

use crate::{FfmpegCliBackend, ToolError};

const MAX_DIAGNOSTIC_BYTES: usize = 4 * 1024 * 1024;

impl FfmpegCliBackend {
    pub(crate) async fn inspect_capabilities(
        &self,
        request: CapabilityRequest,
        cancel: CancellationToken,
    ) -> Result<BackendCapabilities, BackendError> {
        let ffmpeg_version = run("ffmpeg", self.ffmpeg_path(), &["-version"], cancel.clone())
            .await
            .map_err(capability_error)?;
        let ffprobe_version = run(
            "ffprobe",
            self.ffprobe_path(),
            &["-version"],
            cancel.clone(),
        )
        .await
        .map_err(capability_error)?;
        let decoders = run(
            "ffmpeg",
            self.ffmpeg_path(),
            &["-hide_banner", "-decoders"],
            cancel.clone(),
        )
        .await
        .map_err(capability_error)?;
        let encoders = run(
            "ffmpeg",
            self.ffmpeg_path(),
            &["-hide_banner", "-encoders"],
            cancel.clone(),
        )
        .await
        .map_err(capability_error)?;
        let formats = run(
            "ffmpeg",
            self.ffmpeg_path(),
            &["-hide_banner", "-formats"],
            cancel,
        )
        .await
        .map_err(capability_error)?;

        let checks = request
            .required()
            .iter()
            .cloned()
            .map(|capability| {
                let available = match &capability {
                    MediaCapability::Decoder(name) => codec_present(&decoders, name, "dca"),
                    MediaCapability::Encoder(name) => codec_present(&encoders, name, name),
                    MediaCapability::Demuxer(name) => format_present(&formats, name, 'D'),
                    MediaCapability::Muxer(name) => format_present(&formats, name, 'E'),
                    _ => false,
                };
                CapabilityCheck::new(capability, available, None)
            })
            .collect();
        let mut warnings = Vec::new();
        if self.ffmpeg_path().parent() != self.ffprobe_path().parent() {
            warnings.push("ffmpeg and ffprobe resolve from different directories".to_owned());
        }
        let ffmpeg_line = first_line(&ffmpeg_version);
        let ffprobe_line = first_line(&ffprobe_version);
        if leading_version(ffmpeg_line.as_deref()) != leading_version(ffprobe_line.as_deref()) {
            warnings.push("ffmpeg and ffprobe report different leading versions".to_owned());
        }
        Ok(BackendCapabilities::new(
            "ffmpeg-cli",
            vec![
                BackendToolInfo::new(
                    BackendToolRole::Ffmpeg,
                    self.ffmpeg_path().to_path_buf(),
                    ffmpeg_line,
                ),
                BackendToolInfo::new(
                    BackendToolRole::Ffprobe,
                    self.ffprobe_path().to_path_buf(),
                    ffprobe_line,
                ),
            ],
            checks,
            warnings,
        ))
    }
}

fn capability_error(source: ToolError) -> BackendError {
    match source {
        ToolError::Cancelled => BackendError::Cancelled,
        source => BackendError::Capability {
            source: Box::new(source),
        },
    }
}

async fn run(
    name: &'static str,
    executable: &Path,
    args: &[&str],
    cancel: CancellationToken,
) -> Result<String, ToolError> {
    if cancel.is_cancelled() {
        return Err(ToolError::Cancelled);
    }
    let mut command = Command::new(executable);
    command
        .args(args)
        .stdin(Stdio::null())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped());
    let mut group = command.group();
    group.kill_on_drop(true);
    #[cfg(windows)]
    group.creation_flags(0x0800_0000);
    let mut child = group.spawn().map_err(|source| ToolError::Spawn {
        name,
        path: executable.to_path_buf(),
        source,
    })?;
    let stdout = child
        .inner()
        .stdout
        .take()
        .ok_or_else(|| protocol("diagnostic stdout pipe was unavailable"))?;
    let stderr = child
        .inner()
        .stderr
        .take()
        .ok_or_else(|| protocol("diagnostic stderr pipe was unavailable"))?;
    let stdout_task = tokio::spawn(read_limited(stdout));
    let stderr_task = tokio::spawn(read_limited(stderr));
    let status = tokio::select! {
        () = cancel.cancelled() => {
            terminate_and_reap(name, &mut child).await?;
            let _stdout = stdout_task.await;
            let _stderr = stderr_task.await;
            return Err(ToolError::Cancelled);
        }
        result = child.wait() => result.map_err(|source| ToolError::DiagnosticIo {
            name,
            operation: "wait",
            source,
        })?,
    };
    let stdout = join_reader(name, stdout_task.await)?;
    let stderr = join_reader(name, stderr_task.await)?;
    if stdout.len().saturating_add(stderr.len()) > MAX_DIAGNOSTIC_BYTES {
        return Err(ToolError::Protocol {
            message: "diagnostic output exceeded the 4 MiB limit".to_owned(),
        });
    }
    if !status.success() {
        return Err(ToolError::DiagnosticFailed {
            name,
            stderr: bounded(&String::from_utf8_lossy(&stderr)),
        });
    }
    Ok(String::from_utf8_lossy(&stdout).into_owned())
}

async fn read_limited<R>(reader: R) -> io::Result<Vec<u8>>
where
    R: tokio::io::AsyncRead + Unpin,
{
    let mut bytes = Vec::new();
    reader
        .take((MAX_DIAGNOSTIC_BYTES + 1) as u64)
        .read_to_end(&mut bytes)
        .await?;
    Ok(bytes)
}

fn join_reader(
    name: &'static str,
    result: Result<io::Result<Vec<u8>>, tokio::task::JoinError>,
) -> Result<Vec<u8>, ToolError> {
    match result {
        Ok(Ok(bytes)) => Ok(bytes),
        Ok(Err(source)) => Err(ToolError::DiagnosticIo {
            name,
            operation: "read",
            source,
        }),
        Err(error) => Err(protocol(&format!("diagnostic reader task failed: {error}"))),
    }
}

async fn terminate_and_reap(
    name: &'static str,
    child: &mut AsyncGroupChild,
) -> Result<(), ToolError> {
    if child
        .try_wait()
        .map_err(|source| ToolError::DiagnosticIo {
            name,
            operation: "poll",
            source,
        })?
        .is_some()
    {
        return Ok(());
    }
    let kill_error = child.start_kill().err();
    child
        .wait()
        .await
        .map_err(|source| ToolError::DiagnosticIo {
            name,
            operation: "reap",
            source,
        })?;
    if let Some(source) = kill_error {
        return Err(ToolError::DiagnosticIo {
            name,
            operation: "terminate",
            source,
        });
    }
    Ok(())
}

fn protocol(message: &str) -> ToolError {
    ToolError::Protocol {
        message: message.to_owned(),
    }
}

fn first_line(output: &str) -> Option<String> {
    output
        .lines()
        .map(str::trim)
        .find(|line| !line.is_empty())
        .map(bounded)
}

fn leading_version(line: Option<&str>) -> Option<&str> {
    line.and_then(|value| value.split_whitespace().nth(2))
}

fn codec_present(output: &str, requested: &str, dts_alias: &str) -> bool {
    let expected = if requested.eq_ignore_ascii_case("dts") {
        dts_alias
    } else {
        requested
    };
    output.lines().any(|line| {
        let mut fields = line.split_whitespace();
        let flags = fields.next().unwrap_or_default();
        let name = fields.next().unwrap_or_default();
        flags.len() >= 6 && name.eq_ignore_ascii_case(expected)
    })
}

fn format_present(output: &str, requested: &str, flag: char) -> bool {
    output.lines().any(|line| {
        let mut fields = line.split_whitespace();
        let flags = fields.next().unwrap_or_default();
        let names = fields.next().unwrap_or_default();
        flags.contains(flag)
            && names
                .split(',')
                .any(|name| name.eq_ignore_ascii_case(requested))
    })
}

fn bounded(value: &str) -> String {
    value.chars().take(512).collect()
}

#[cfg(test)]
mod tests {
    use super::{codec_present, format_present};

    #[test]
    fn parses_codec_and_format_tables() {
        assert!(codec_present(" A....D dca DTS", "dts", "dca"));
        assert!(format_present(
            " DE matroska,webm Matroska",
            "matroska",
            'D'
        ));
        assert!(format_present(
            " DE matroska,webm Matroska",
            "matroska",
            'E'
        ));
    }
}
