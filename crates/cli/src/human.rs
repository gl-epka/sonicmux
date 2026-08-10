//! Stable human-readable rendering kept separate from domain logic.

use sonicmux_backend::BackendCapabilities;
use sonicmux_core::{JobPlan, MediaInfo, StreamInfo};

/// Renders one media inspection report.
#[must_use]
pub fn probe(media: &MediaInfo, compact: bool) -> String {
    if compact {
        return format!(
            "{}: {} stream(s), {} audio, {} chapter(s)",
            media.path().display(),
            media.streams().len(),
            media.audio_streams().count(),
            media.chapters().len()
        );
    }
    let mut output = format!(
        "{}\n  container: {}\n  duration: {}\n  chapters: {}\n",
        media.path().display(),
        media.format().names().join(", "),
        media.format().duration().map_or_else(
            || "unknown".to_owned(),
            |value| format!("{} us", value.get())
        ),
        media.chapters().len()
    );
    for stream in media.streams() {
        let common = stream.common();
        let (kind, details) = match stream {
            StreamInfo::Audio(audio) => (
                "audio",
                format!(
                    "{}; {} ch{}",
                    audio.codec(),
                    audio.channels().count(),
                    audio
                        .channels()
                        .layout_name()
                        .map_or_else(String::new, |layout| format!(" {layout}"))
                ),
            ),
            StreamInfo::Video(_) => ("video", common.codec_name().to_owned()),
            StreamInfo::Subtitle(_) => ("subtitle", common.codec_name().to_owned()),
            StreamInfo::Attachment(_) => ("attachment", common.codec_name().to_owned()),
            StreamInfo::Data(_) => ("data", common.codec_name().to_owned()),
            StreamInfo::Unknown(value) => (value.kind(), common.codec_name().to_owned()),
            _ => ("unknown", common.codec_name().to_owned()),
        };
        output.push_str(&format!(
            "  stream {}: {kind}; {details}; language={}; title={}; default={}\n",
            stream.index(),
            common
                .metadata()
                .language()
                .map_or_else(|| "-".to_owned(), |value| value.to_string()),
            common.metadata().title().unwrap_or("-"),
            common.dispositions().is_default()
        ));
    }
    output.trim_end().to_owned()
}

/// Renders one executable plan.
#[must_use]
pub fn plan(plan: &JobPlan) -> String {
    format!(
        "{} -> {}: {:?}, {} stream(s), {} encoded audio stream(s)",
        plan.input().display(),
        plan.output().display(),
        plan.action(),
        plan.streams().len(),
        plan.streams()
            .iter()
            .filter(|stream| stream.is_encode())
            .count()
    )
}

/// Renders a backend diagnostic report.
#[must_use]
pub fn doctor(report: &BackendCapabilities, print_paths: bool) -> String {
    let mut output = format!("Backend: {}\n", report.backend_name());
    for tool in report.tools() {
        output.push_str(&format!(
            "  {:?}: {}{}\n",
            tool.role(),
            tool.version().unwrap_or("unknown version"),
            if print_paths {
                format!(" ({})", tool.path().display())
            } else {
                String::new()
            }
        ));
    }
    for check in report.checks() {
        output.push_str(&format!(
            "  [{}] {} {}\n",
            if check.available() { "ok" } else { "missing" },
            check.capability().kind(),
            check.capability().name()
        ));
    }
    for warning in report.warnings() {
        output.push_str(&format!("  warning: {warning}\n"));
    }
    output.trim_end().to_owned()
}
