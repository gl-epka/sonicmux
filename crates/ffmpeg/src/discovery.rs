//! Cross-platform discovery of a matching FFmpeg and FFprobe pair.

use std::path::{Path, PathBuf};

use crate::{FfmpegToolchainPaths, ToolError};

/// Resolves an explicit FFmpeg path/directory or falls back to `PATH`.
///
/// # Errors
///
/// Returns [`ToolError`] when either member of the pair cannot be resolved.
pub fn resolve_toolchain(explicit: Option<&Path>) -> Result<FfmpegToolchainPaths, ToolError> {
    match explicit {
        Some(path) => resolve_explicit(path),
        None => resolve_path(),
    }
}

fn resolve_explicit(path: &Path) -> Result<FfmpegToolchainPaths, ToolError> {
    let (ffmpeg, ffprobe) = if path.is_dir() {
        (path.join(ffmpeg_name()), path.join(ffprobe_name()))
    } else {
        let file_name = path.file_name().and_then(|value| value.to_str());
        if !file_name.is_some_and(|value| value.eq_ignore_ascii_case(ffmpeg_name())) {
            return Err(ToolError::InvalidPath {
                path: path.to_path_buf(),
            });
        }
        let parent = path.parent().unwrap_or_else(|| Path::new("."));
        (path.to_path_buf(), parent.join(ffprobe_name()))
    };
    let ffmpeg = require_file("ffmpeg", ffmpeg)?;
    let ffprobe = require_file("ffprobe", ffprobe)?;
    Ok(FfmpegToolchainPaths::new(ffmpeg, ffprobe))
}

fn resolve_path() -> Result<FfmpegToolchainPaths, ToolError> {
    let ffmpeg =
        which::which(ffmpeg_name()).map_err(|_| ToolError::PathLookup { name: "ffmpeg" })?;
    let ffprobe =
        which::which(ffprobe_name()).map_err(|_| ToolError::PathLookup { name: "ffprobe" })?;
    Ok(FfmpegToolchainPaths::new(ffmpeg, ffprobe))
}

fn require_file(name: &'static str, path: PathBuf) -> Result<PathBuf, ToolError> {
    if path.is_file() {
        Ok(path)
    } else {
        Err(ToolError::ExecutableNotFound { name, path })
    }
}

#[cfg(windows)]
const fn ffmpeg_name() -> &'static str {
    "ffmpeg.exe"
}

#[cfg(not(windows))]
const fn ffmpeg_name() -> &'static str {
    "ffmpeg"
}

#[cfg(windows)]
const fn ffprobe_name() -> &'static str {
    "ffprobe.exe"
}

#[cfg(not(windows))]
const fn ffprobe_name() -> &'static str {
    "ffprobe"
}

#[cfg(test)]
mod tests {
    use super::{ffmpeg_name, resolve_toolchain};

    #[test]
    fn rejects_an_unrelated_explicit_filename() {
        let error = resolve_toolchain(Some(std::path::Path::new("not-ffmpeg")));
        assert!(error.is_err());
    }

    #[test]
    fn platform_name_is_not_empty() {
        assert!(!ffmpeg_name().is_empty());
    }
}
