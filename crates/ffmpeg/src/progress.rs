//! Bounded FFmpeg `-progress` protocol parsing.

use std::{io, str};

use sonicmux_backend::{ProgressEvent, ProgressSnapshot};
use thiserror::Error;
use tokio::{io::AsyncRead, io::AsyncReadExt as _, sync::mpsc};

const MAX_PROGRESS_LINE_BYTES: usize = 16 * 1024;

/// Progress protocol failure.
#[derive(Debug, Error)]
#[non_exhaustive]
pub enum ProgressError {
    /// Reading the progress pipe failed.
    #[error("failed to read FFmpeg progress: {source}")]
    Read {
        /// Operating-system error.
        #[source]
        source: io::Error,
    },
    /// One protocol line exceeded the bounded parser limit.
    #[error("FFmpeg progress line exceeds {limit_bytes} bytes")]
    LineTooLong {
        /// Configured safety limit.
        limit_bytes: usize,
    },
    /// Progress was not UTF-8.
    #[error("FFmpeg progress line is not valid UTF-8")]
    InvalidUtf8,
    /// A known key contained an invalid value.
    #[error("invalid FFmpeg progress value for `{key}`")]
    InvalidValue {
        /// Stable key name; the untrusted value is intentionally omitted.
        key: &'static str,
    },
    /// A record appeared after the terminal record.
    #[error("FFmpeg progress contained data after `progress=end`")]
    DataAfterEnd,
}

/// Result of consuming one complete progress stream.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct ProgressReadReport {
    pub(crate) last: Option<ProgressSnapshot>,
    pub(crate) saw_end: bool,
}

#[derive(Debug, Default)]
struct ProgressParser {
    current: ProgressSnapshot,
    last: Option<ProgressSnapshot>,
    saw_end: bool,
}

impl ProgressParser {
    fn push_line(
        &mut self,
        line: &[u8],
        sender: &mpsc::Sender<ProgressEvent>,
    ) -> Result<(), ProgressError> {
        let line = line.strip_suffix(b"\r").unwrap_or(line);
        if line.is_empty() {
            return Ok(());
        }
        if self.saw_end {
            return Err(ProgressError::DataAfterEnd);
        }
        let line = str::from_utf8(line).map_err(|_| ProgressError::InvalidUtf8)?;
        let Some((key, value)) = line.split_once('=') else {
            tracing::trace!("ignoring FFmpeg progress line without separator");
            return Ok(());
        };
        let value = value.trim();
        match key {
            "out_time_us" => self.current.out_time_us = parse_optional(value, "out_time_us")?,
            "total_size" => {
                self.current.total_size_bytes = parse_optional(value, "total_size")?;
            }
            "frame" => self.current.frame = parse_optional(value, "frame")?,
            "drop_frames" => {
                self.current.dropped_frames = parse_optional(value, "drop_frames")?;
            }
            "speed" => self.current.speed_milli = parse_speed(value)?,
            "progress" => {
                let snapshot = self.current.clone();
                let event = match value {
                    "continue" => ProgressEvent::Advanced(snapshot.clone()),
                    "end" => {
                        self.saw_end = true;
                        ProgressEvent::Finished(snapshot.clone())
                    }
                    _ => return Err(ProgressError::InvalidValue { key: "progress" }),
                };
                self.last = Some(snapshot);
                let _dropped = sender.try_send(event);
                self.current = ProgressSnapshot::default();
            }
            _ => tracing::trace!(progress_key = key, "ignoring FFmpeg progress key"),
        }
        Ok(())
    }
}

fn parse_optional<T>(value: &str, key: &'static str) -> Result<Option<T>, ProgressError>
where
    T: str::FromStr,
{
    if value == "N/A" {
        return Ok(None);
    }
    value
        .parse()
        .map(Some)
        .map_err(|_| ProgressError::InvalidValue { key })
}

fn parse_speed(value: &str) -> Result<Option<u32>, ProgressError> {
    if value == "N/A" {
        return Ok(None);
    }
    let decimal = value
        .strip_suffix('x')
        .ok_or(ProgressError::InvalidValue { key: "speed" })?;
    let (mantissa, exponent) =
        decimal
            .split_once(['e', 'E'])
            .map_or((decimal, 0_i32), |(mantissa, exponent)| {
                let exponent = exponent.parse::<i32>().unwrap_or(i32::MIN);
                (mantissa, exponent)
            });
    if exponent == i32::MIN {
        return Err(ProgressError::InvalidValue { key: "speed" });
    }
    let (whole, fraction) = mantissa.split_once('.').unwrap_or((mantissa, ""));
    if whole.is_empty()
        || !whole.bytes().all(|byte| byte.is_ascii_digit())
        || !fraction.bytes().all(|byte| byte.is_ascii_digit())
    {
        return Err(ProgressError::InvalidValue { key: "speed" });
    }
    let digits = format!("{whole}{fraction}")
        .parse::<u128>()
        .map_err(|_| ProgressError::InvalidValue { key: "speed" })?;
    let power = exponent
        .checked_add(3)
        .and_then(|value| value.checked_sub(i32::try_from(fraction.len()).ok()?))
        .ok_or(ProgressError::InvalidValue { key: "speed" })?;
    let scaled = if power >= 0 {
        digits.checked_mul(
            10_u128
                .checked_pow(power.unsigned_abs())
                .ok_or(ProgressError::InvalidValue { key: "speed" })?,
        )
    } else {
        Some(
            digits
                / 10_u128
                    .checked_pow(power.unsigned_abs())
                    .ok_or(ProgressError::InvalidValue { key: "speed" })?,
        )
    };
    let milli = scaled
        .and_then(|value| u32::try_from(value).ok())
        .ok_or(ProgressError::InvalidValue { key: "speed" })?;
    Ok(Some(milli))
}

pub(crate) async fn read_progress<R>(
    mut reader: R,
    sender: mpsc::Sender<ProgressEvent>,
) -> Result<ProgressReadReport, ProgressError>
where
    R: AsyncRead + Unpin,
{
    let mut parser = ProgressParser::default();
    let mut chunk = [0_u8; 4096];
    let mut line = Vec::new();
    loop {
        let read = reader
            .read(&mut chunk)
            .await
            .map_err(|source| ProgressError::Read { source })?;
        if read == 0 {
            break;
        }
        for byte in &chunk[..read] {
            if *byte == b'\n' {
                parser.push_line(&line, &sender)?;
                line.clear();
            } else {
                if line.len() == MAX_PROGRESS_LINE_BYTES {
                    return Err(ProgressError::LineTooLong {
                        limit_bytes: MAX_PROGRESS_LINE_BYTES,
                    });
                }
                line.push(*byte);
            }
        }
    }
    if !line.is_empty() {
        parser.push_line(&line, &sender)?;
    }
    Ok(ProgressReadReport {
        last: parser.last,
        saw_end: parser.saw_end,
    })
}

#[cfg(test)]
#[allow(clippy::expect_used, clippy::panic)]
mod tests {
    use sonicmux_backend::ProgressEvent;
    use tokio::sync::mpsc;

    use super::{ProgressError, read_progress};

    #[tokio::test]
    async fn parses_partial_crlf_records_and_fixed_speed() {
        let input = b"out_time_us=-21000\r\ntotal_size=42\r\nspeed=1.37x\r\nprogress=continue\r\nout_time_us=5000000\r\nprogress=end\r\n";
        let (sender, mut receiver) = mpsc::channel(4);
        let report = read_progress(&input[..], sender)
            .await
            .expect("progress parses");
        assert!(report.saw_end);
        let first = receiver.recv().await.expect("first event exists");
        match first {
            ProgressEvent::Advanced(snapshot) => {
                assert_eq!(snapshot.out_time_us, Some(-21_000));
                assert_eq!(snapshot.total_size_bytes, Some(42));
                assert_eq!(snapshot.speed_milli, Some(1_370));
            }
            _ => panic!("unexpected progress event"),
        }
    }

    #[tokio::test]
    async fn parses_scientific_speed_from_short_jobs() {
        let input = b"speed= 1.25e+03x \nprogress=end\n";
        let (sender, _receiver) = mpsc::channel(1);
        let report = read_progress(&input[..], sender)
            .await
            .expect("progress parses");
        assert_eq!(
            report.last.expect("snapshot exists").speed_milli,
            Some(1_250_000)
        );
    }

    #[tokio::test]
    async fn accepts_na_and_ignores_unknown_keys() {
        let input = b"out_time_us=N/A\nfuture_key=value\nspeed=N/A\nprogress=end\n";
        let (sender, _receiver) = mpsc::channel(1);
        let report = read_progress(&input[..], sender)
            .await
            .expect("progress parses");
        let last = report.last.expect("last snapshot exists");
        assert_eq!(last.out_time_us, None);
        assert_eq!(last.speed_milli, None);
    }

    #[tokio::test]
    async fn full_progress_channel_does_not_block_reader() {
        let input = b"progress=continue\nprogress=end\n";
        let (sender, _receiver) = mpsc::channel(1);
        let report = read_progress(&input[..], sender)
            .await
            .expect("reader completes");
        assert!(report.saw_end);
    }

    #[tokio::test]
    async fn rejects_oversized_line_before_unbounded_growth() {
        let input = vec![b'a'; 16 * 1024 + 1];
        let (sender, _receiver) = mpsc::channel(1);
        let error = read_progress(&input[..], sender)
            .await
            .expect_err("line must fail");
        assert!(matches!(error, ProgressError::LineTooLong { .. }));
    }

    #[tokio::test]
    async fn rejects_malformed_known_value_without_echoing_it() {
        let input = b"frame=not-a-number\n";
        let (sender, _receiver) = mpsc::channel(1);
        let error = read_progress(&input[..], sender)
            .await
            .expect_err("value must fail");
        assert_eq!(
            error.to_string(),
            "invalid FFmpeg progress value for `frame`"
        );
    }
}
