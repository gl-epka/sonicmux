//! Deterministic MKV file, directory, and glob discovery.

use std::{
    collections::BTreeSet,
    ffi::OsString,
    path::{Path, PathBuf},
};

use glob::MatchOptions;
use globset::{GlobBuilder, GlobSet, GlobSetBuilder};
use thiserror::Error;
use walkdir::WalkDir;

/// One discovery request with explicit traversal policy.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct DiscoveryRequest {
    /// Literal paths or Unicode glob spellings.
    pub roots: Vec<OsString>,
    /// Descend below direct directory children.
    pub recursive: bool,
    /// Follow explicit and encountered symbolic links.
    pub follow_links: bool,
    /// Relative include patterns.
    pub includes: Vec<String>,
    /// Relative exclude patterns.
    pub excludes: Vec<String>,
}

/// Input discovery failure.
#[derive(Debug, Error)]
#[non_exhaustive]
pub enum DiscoveryError {
    /// No input roots were supplied.
    #[error("at least one input path is required")]
    Empty,
    /// A glob expression is invalid.
    #[error("invalid {kind} glob `{pattern}`: {reason}")]
    InvalidGlob {
        /// Pattern category.
        kind: &'static str,
        /// Rejected pattern.
        pattern: String,
        /// Parser explanation.
        reason: String,
    },
    /// An explicit glob matched nothing.
    #[error("input glob matched no paths: {pattern}")]
    UnmatchedGlob {
        /// Original expression.
        pattern: String,
    },
    /// A root could not be inspected.
    #[error("failed to inspect input {}: {source}", path.display())]
    Inspect {
        /// Input path.
        path: PathBuf,
        /// Operating-system error.
        #[source]
        source: std::io::Error,
    },
    /// Symbolic link traversal was not explicitly enabled.
    #[error("symbolic-link input requires --follow-links: {}", path.display())]
    Symlink {
        /// Rejected path.
        path: PathBuf,
    },
    /// An input is neither a regular MKV file nor a directory.
    #[error("input is not an MKV file or directory: {}", path.display())]
    Unsupported {
        /// Rejected path.
        path: PathBuf,
    },
    /// Directory traversal failed.
    #[error("failed while traversing {}: {reason}", path.display())]
    Traverse {
        /// Discovery root.
        path: PathBuf,
        /// Bounded walk diagnostic.
        reason: String,
    },
    /// A blocking discovery task failed.
    #[error("input discovery task failed: {reason}")]
    Task {
        /// Join diagnostic.
        reason: String,
    },
}

/// Discovers MKV files on the blocking filesystem pool.
pub async fn discover(request: DiscoveryRequest) -> Result<Vec<PathBuf>, DiscoveryError> {
    tokio::task::spawn_blocking(move || discover_blocking(&request))
        .await
        .map_err(|error| DiscoveryError::Task {
            reason: error.to_string(),
        })?
}

/// Performs deterministic discovery synchronously.
pub fn discover_blocking(request: &DiscoveryRequest) -> Result<Vec<PathBuf>, DiscoveryError> {
    if request.roots.is_empty() {
        return Err(DiscoveryError::Empty);
    }
    let includes = compile_set("include", &request.includes)?;
    let excludes = compile_set("exclude", &request.excludes)?;
    let mut paths = BTreeSet::new();
    for root in &request.roots {
        if let Some(pattern) = root.to_str().filter(|value| has_glob(value)) {
            let entries = glob::glob_with(
                pattern,
                MatchOptions {
                    case_sensitive: cfg!(not(windows)),
                    require_literal_separator: false,
                    require_literal_leading_dot: false,
                },
            )
            .map_err(|error| DiscoveryError::InvalidGlob {
                kind: "input",
                pattern: pattern.to_owned(),
                reason: error.to_string(),
            })?;
            let mut matched = false;
            for entry in entries {
                matched = true;
                let path = entry.map_err(|error| DiscoveryError::Inspect {
                    path: error.path().to_path_buf(),
                    source: std::io::Error::new(error.error().kind(), error.error().to_string()),
                })?;
                collect_root(&path, request, &includes, &excludes, &mut paths)?;
            }
            if !matched {
                return Err(DiscoveryError::UnmatchedGlob {
                    pattern: pattern.to_owned(),
                });
            }
        } else {
            collect_root(Path::new(root), request, &includes, &excludes, &mut paths)?;
        }
    }
    Ok(paths.into_iter().collect())
}

fn collect_root(
    root: &Path,
    request: &DiscoveryRequest,
    includes: &GlobSet,
    excludes: &GlobSet,
    output: &mut BTreeSet<PathBuf>,
) -> Result<(), DiscoveryError> {
    let link_metadata =
        std::fs::symlink_metadata(root).map_err(|source| DiscoveryError::Inspect {
            path: root.to_path_buf(),
            source,
        })?;
    if link_metadata.file_type().is_symlink() && !request.follow_links {
        return Err(DiscoveryError::Symlink {
            path: root.to_path_buf(),
        });
    }
    let metadata = if link_metadata.file_type().is_symlink() {
        std::fs::metadata(root).map_err(|source| DiscoveryError::Inspect {
            path: root.to_path_buf(),
            source,
        })?
    } else {
        link_metadata
    };
    if metadata.is_file() {
        if !is_mkv(root) {
            return Err(DiscoveryError::Unsupported {
                path: root.to_path_buf(),
            });
        }
        let relative = root.file_name().map_or_else(|| Path::new(""), Path::new);
        if matches_filters(relative, includes, excludes) {
            output.insert(root.to_path_buf());
        }
        return Ok(());
    }
    if !metadata.is_dir() {
        return Err(DiscoveryError::Unsupported {
            path: root.to_path_buf(),
        });
    }
    let max_depth = if request.recursive { usize::MAX } else { 1 };
    for entry in WalkDir::new(root)
        .follow_links(request.follow_links)
        .max_depth(max_depth)
        .min_depth(1)
    {
        let entry = entry.map_err(|error| DiscoveryError::Traverse {
            path: root.to_path_buf(),
            reason: error.to_string(),
        })?;
        if entry.file_type().is_file() && is_mkv(entry.path()) {
            let relative = entry
                .path()
                .strip_prefix(root)
                .unwrap_or_else(|_| entry.path());
            if matches_filters(relative, includes, excludes) {
                output.insert(entry.path().to_path_buf());
            }
        }
    }
    Ok(())
}

fn compile_set(kind: &'static str, patterns: &[String]) -> Result<GlobSet, DiscoveryError> {
    let patterns: Vec<&str> = if patterns.is_empty() && kind == "include" {
        vec!["*.mkv"]
    } else {
        patterns.iter().map(String::as_str).collect()
    };
    let mut builder = GlobSetBuilder::new();
    for pattern in patterns {
        let glob = GlobBuilder::new(pattern)
            .case_insensitive(true)
            .literal_separator(false)
            .build()
            .map_err(|error| DiscoveryError::InvalidGlob {
                kind,
                pattern: pattern.to_owned(),
                reason: error.to_string(),
            })?;
        builder.add(glob);
    }
    builder
        .build()
        .map_err(|error| DiscoveryError::InvalidGlob {
            kind,
            pattern: "<set>".to_owned(),
            reason: error.to_string(),
        })
}

fn matches_filters(path: &Path, includes: &GlobSet, excludes: &GlobSet) -> bool {
    includes.is_match(path) && !excludes.is_match(path)
}

fn is_mkv(path: &Path) -> bool {
    path.extension()
        .and_then(|value| value.to_str())
        .is_some_and(|value| value.eq_ignore_ascii_case("mkv"))
}

fn has_glob(value: &str) -> bool {
    value.bytes().any(|byte| matches!(byte, b'*' | b'?' | b'['))
}

#[cfg(test)]
#[allow(clippy::expect_used, clippy::panic)]
mod tests {
    use std::{ffi::OsString, fs};

    use tempfile::tempdir;

    use super::{DiscoveryRequest, discover_blocking};

    #[test]
    fn sorts_deduplicates_and_filters_mkv_files() {
        let directory = tempdir().expect("tempdir");
        fs::write(directory.path().join("b.MKV"), []).expect("fixture");
        fs::write(directory.path().join("a.mkv"), []).expect("fixture");
        fs::write(directory.path().join("skip.mp4"), []).expect("fixture");
        let files = discover_blocking(&DiscoveryRequest {
            roots: vec![
                OsString::from(directory.path()),
                OsString::from(directory.path()),
            ],
            recursive: false,
            follow_links: false,
            includes: Vec::new(),
            excludes: Vec::new(),
        })
        .expect("discovery succeeds");
        assert_eq!(files.len(), 2);
        assert!(files[0].ends_with("a.mkv"));
    }

    #[test]
    fn recursive_depth_and_unmatched_globs_are_explicit() {
        let directory = tempdir().expect("tempdir");
        let nested = directory.path().join("nested");
        fs::create_dir(&nested).expect("nested directory");
        fs::write(nested.join("inside.mkv"), []).expect("fixture");
        let shallow = discover_blocking(&DiscoveryRequest {
            roots: vec![OsString::from(directory.path())],
            recursive: false,
            follow_links: false,
            includes: Vec::new(),
            excludes: Vec::new(),
        })
        .expect("shallow discovery succeeds");
        assert!(shallow.is_empty());
        let deep = discover_blocking(&DiscoveryRequest {
            roots: vec![OsString::from(directory.path())],
            recursive: true,
            follow_links: false,
            includes: Vec::new(),
            excludes: Vec::new(),
        })
        .expect("recursive discovery succeeds");
        assert_eq!(deep, vec![nested.join("inside.mkv")]);

        let unmatched = directory.path().join("*.missing.mkv");
        let error = discover_blocking(&DiscoveryRequest {
            roots: vec![unmatched.into_os_string()],
            recursive: false,
            follow_links: false,
            includes: Vec::new(),
            excludes: Vec::new(),
        });
        assert!(matches!(
            error,
            Err(super::DiscoveryError::UnmatchedGlob { .. })
        ));
    }
}
