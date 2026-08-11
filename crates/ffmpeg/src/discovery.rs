//! Cross-platform discovery of a matching FFmpeg and FFprobe pair.

use std::path::{Path, PathBuf};

use crate::{FfmpegToolchainPaths, ToolError};

/// Origin of a successfully resolved executable pair.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ToolchainSource {
    /// A user, environment, or configuration value selected the pair.
    Explicit,
    /// A GUI application bundle supplied the pair.
    Bundled,
    /// Both executables were discovered through the process `PATH`.
    Path,
}

impl ToolchainSource {
    /// Returns the stable machine spelling.
    #[must_use]
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::Explicit => "explicit",
            Self::Bundled => "bundled",
            Self::Path => "path",
        }
    }
}

/// A matching executable pair and the source that selected it.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ResolvedToolchain {
    paths: FfmpegToolchainPaths,
    source: ToolchainSource,
}

impl ResolvedToolchain {
    /// Returns the executable paths.
    #[must_use]
    pub const fn paths(&self) -> &FfmpegToolchainPaths {
        &self.paths
    }

    /// Returns the winning discovery source.
    #[must_use]
    pub const fn source(&self) -> ToolchainSource {
        self.source
    }

    /// Consumes the result and returns its executable paths.
    #[must_use]
    pub fn into_paths(self) -> FfmpegToolchainPaths {
        self.paths
    }
}

/// Resolves an explicit FFmpeg path/directory or falls back to `PATH`.
///
/// # Errors
///
/// Returns [`ToolError`] when either member of the pair cannot be resolved.
pub fn resolve_toolchain(explicit: Option<&Path>) -> Result<FfmpegToolchainPaths, ToolError> {
    resolve_toolchain_hybrid(explicit, None).map(ResolvedToolchain::into_paths)
}

/// Resolves an explicit pair, an optional GUI sidecar directory, or `PATH`.
///
/// An invalid explicit value is authoritative and is returned directly. An
/// absent or incomplete bundle falls back to `PATH`, which lets unpackaged GUI
/// development builds use a system installation.
///
/// # Errors
///
/// Returns [`ToolError`] when the authoritative explicit value is invalid or
/// no complete fallback pair can be resolved.
pub fn resolve_toolchain_hybrid(
    explicit: Option<&Path>,
    bundled_directory: Option<&Path>,
) -> Result<ResolvedToolchain, ToolError> {
    if let Some(path) = explicit {
        return resolve_explicit(path).map(|paths| ResolvedToolchain {
            paths,
            source: ToolchainSource::Explicit,
        });
    }
    if let Some(directory) = bundled_directory
        && let Ok(paths) = resolve_explicit(directory)
    {
        return Ok(ResolvedToolchain {
            paths,
            source: ToolchainSource::Bundled,
        });
    }
    resolve_path().map(|paths| ResolvedToolchain {
        paths,
        source: ToolchainSource::Path,
    })
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
    use std::{fs, path::Path};

    use tempfile::tempdir;

    use super::{
        ToolchainSource, ffmpeg_name, ffprobe_name, resolve_toolchain, resolve_toolchain_hybrid,
    };

    #[test]
    fn rejects_an_unrelated_explicit_filename() {
        let error = resolve_toolchain(Some(std::path::Path::new("not-ffmpeg")));
        assert!(error.is_err());
    }

    #[test]
    fn platform_name_is_not_empty() {
        assert!(!ffmpeg_name().is_empty());
    }

    #[test]
    fn complete_bundle_wins_and_incomplete_bundle_falls_back()
    -> Result<(), Box<dyn std::error::Error>> {
        let directory = tempdir()?;
        fs::write(directory.path().join(ffmpeg_name()), [])?;
        fs::write(directory.path().join(ffprobe_name()), [])?;

        let resolved = resolve_toolchain_hybrid(None, Some(directory.path()))?;
        assert_eq!(resolved.source(), ToolchainSource::Bundled);

        let incomplete = tempdir()?;
        let result = resolve_toolchain_hybrid(None, Some(incomplete.path()));
        if let Ok(resolved) = result {
            assert_eq!(resolved.source(), ToolchainSource::Path);
        }
        Ok(())
    }

    #[test]
    fn explicit_value_never_silently_falls_back() {
        let error = resolve_toolchain_hybrid(Some(Path::new("missing-ffmpeg")), None);
        assert!(error.is_err());
    }
}
