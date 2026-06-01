use serde::Serialize;

pub(crate) const OUTPUT_TRUNCATION_MARKER: &[u8] = b"\n...[output truncated]...\n";

#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct ProjectedOutput {
    pub content: String,
    pub original_bytes: usize,
    pub truncated: bool,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub(crate) struct OutputProjectionMeta {
    pub strategy: &'static str,
    pub max_bytes: usize,
    pub marker: &'static str,
}

pub(crate) fn output_projection_meta(max_bytes: usize) -> OutputProjectionMeta {
    OutputProjectionMeta {
        strategy: "head_tail_bytes",
        max_bytes,
        marker: std::str::from_utf8(OUTPUT_TRUNCATION_MARKER).unwrap_or(""),
    }
}

pub(crate) fn project_output(bytes: &[u8], max_bytes: usize) -> ProjectedOutput {
    if bytes.len() <= max_bytes {
        return ProjectedOutput {
            content: String::from_utf8_lossy(bytes).to_string(),
            original_bytes: bytes.len(),
            truncated: false,
        };
    }

    if max_bytes <= OUTPUT_TRUNCATION_MARKER.len() {
        return ProjectedOutput {
            content: String::from_utf8_lossy(&bytes[..max_bytes]).to_string(),
            original_bytes: bytes.len(),
            truncated: true,
        };
    }

    let remaining = max_bytes - OUTPUT_TRUNCATION_MARKER.len();
    let head_len = remaining / 2;
    let tail_len = remaining - head_len;
    let mut projected = Vec::with_capacity(max_bytes);
    projected.extend_from_slice(&bytes[..head_len]);
    projected.extend_from_slice(OUTPUT_TRUNCATION_MARKER);
    projected.extend_from_slice(&bytes[bytes.len() - tail_len..]);

    ProjectedOutput {
        content: String::from_utf8_lossy(&projected).to_string(),
        original_bytes: bytes.len(),
        truncated: true,
    }
}
