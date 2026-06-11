use std::collections::hash_map::DefaultHasher;
use std::hash::{Hash, Hasher};
use std::io::Cursor;
use std::num::NonZeroUsize;
use std::path::Path;
use std::sync::{LazyLock, Mutex};

use anyhow::{anyhow, Context, Result};
use base64::engine::general_purpose::STANDARD as BASE64_STANDARD;
use base64::Engine;
use image::imageops::FilterType;
use image::{DynamicImage, GenericImageView, ImageFormat, ImageReader, Limits};
use lru::LruCache;

use crate::types::ImageDetail;

pub const MAX_IMAGE_DIMENSION: u32 = 2048;
pub const MAX_IMAGE_SOURCE_BYTES: usize = 20 * 1024 * 1024;
pub const MAX_IMAGE_SOURCE_DIMENSION: u32 = 8192;
pub const MAX_IMAGE_DECODE_ALLOC_BYTES: u64 = 128 * 1024 * 1024;
const LOW_IMAGE_DIMENSION: u32 = 512;

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct EncodedPromptImage {
    pub mime: String,
    pub base64_data: String,
    pub data_url: String,
    pub width: u32,
    pub height: u32,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
struct ImageCacheKey {
    content_hash: u64,
    detail: ImageDetail,
}

static IMAGE_CACHE: LazyLock<Mutex<LruCache<ImageCacheKey, EncodedPromptImage>>> =
    LazyLock::new(|| {
        Mutex::new(LruCache::new(
            NonZeroUsize::new(32).expect("non-zero cache"),
        ))
    });

pub fn load_local_image_for_prompt(path: &Path, detail: ImageDetail) -> Result<EncodedPromptImage> {
    let metadata = std::fs::metadata(path)
        .with_context(|| format!("could not read image at `{}`", path.display()))?;
    validate_image_source_byte_len(path, metadata.len())?;
    let bytes = std::fs::read(path)
        .with_context(|| format!("could not read image at `{}`", path.display()))?;
    encode_image_bytes_for_prompt(path, bytes, detail)
}

pub fn validate_image_bytes_for_prompt(path: &Path, bytes: &[u8]) -> Result<()> {
    let (format, _, _) = validate_image_header_for_prompt(path, bytes)?;
    decode_image_with_limits(path, bytes, format)?;
    Ok(())
}

pub fn encode_image_bytes_for_prompt(
    path: &Path,
    bytes: Vec<u8>,
    detail: ImageDetail,
) -> Result<EncodedPromptImage> {
    validate_image_source_byte_len(path, bytes.len() as u64)?;
    let key = ImageCacheKey {
        content_hash: hash_bytes(&bytes),
        detail,
    };
    if let Some(cached) = IMAGE_CACHE
        .lock()
        .expect("image cache mutex")
        .get(&key)
        .cloned()
    {
        return Ok(cached);
    }

    let encoded = encode_uncached(path, bytes, detail)?;
    IMAGE_CACHE
        .lock()
        .expect("image cache mutex")
        .put(key, encoded.clone());
    Ok(encoded)
}

fn encode_uncached(path: &Path, bytes: Vec<u8>, detail: ImageDetail) -> Result<EncodedPromptImage> {
    let (format, width, height) = validate_image_header_for_prompt(path, &bytes)?;
    let image = decode_image_with_limits(path, &bytes, format)?;
    let max_dimension = max_dimension_for_detail(detail);
    let within_limit = width <= max_dimension && height <= max_dimension;

    if can_preserve_source_bytes(format) && (within_limit || detail == ImageDetail::Original) {
        let mime = mime_for_format(format).to_string();
        return Ok(encoded_from_bytes(mime, bytes, width, height));
    }

    let output = if detail == ImageDetail::Original {
        image
    } else {
        image.resize(max_dimension, max_dimension, FilterType::Lanczos3)
    };
    let (width, height) = output.dimensions();
    let mut encoded_bytes = Cursor::new(Vec::new());
    output
        .write_to(&mut encoded_bytes, ImageFormat::Png)
        .with_context(|| format!("could not encode image at `{}`", path.display()))?;

    Ok(encoded_from_bytes(
        "image/png".to_string(),
        encoded_bytes.into_inner(),
        width,
        height,
    ))
}

fn validate_image_header_for_prompt(path: &Path, bytes: &[u8]) -> Result<(ImageFormat, u32, u32)> {
    validate_image_source_byte_len(path, bytes.len() as u64)?;
    let format = image::guess_format(bytes)
        .with_context(|| format!("unsupported image at `{}`", path.display()))?;
    if !is_supported_format(format) {
        return Err(anyhow!("unsupported image format at `{}`", path.display()));
    }

    let (width, height) = probe_image_dimensions(path, bytes, format)?;
    validate_image_source_dimensions(path, width, height)?;
    Ok((format, width, height))
}

fn validate_image_source_byte_len(path: &Path, len: u64) -> Result<()> {
    if len > MAX_IMAGE_SOURCE_BYTES as u64 {
        return Err(anyhow!(
            "image at `{}` is {len} bytes, which exceeds the {MAX_IMAGE_SOURCE_BYTES} byte limit",
            path.display()
        ));
    }
    Ok(())
}

fn probe_image_dimensions(path: &Path, bytes: &[u8], format: ImageFormat) -> Result<(u32, u32)> {
    let mut reader = ImageReader::with_format(Cursor::new(bytes), format);
    reader.limits(prompt_image_probe_limits());
    reader
        .into_dimensions()
        .with_context(|| format!("unsupported image at `{}`", path.display()))
}

fn validate_image_source_dimensions(path: &Path, width: u32, height: u32) -> Result<()> {
    if width > MAX_IMAGE_SOURCE_DIMENSION || height > MAX_IMAGE_SOURCE_DIMENSION {
        return Err(anyhow!(
            "image dimensions {width}x{height} at `{}` exceed the {MAX_IMAGE_SOURCE_DIMENSION}x{MAX_IMAGE_SOURCE_DIMENSION} source limit",
            path.display()
        ));
    }
    Ok(())
}

fn decode_image_with_limits(
    path: &Path,
    bytes: &[u8],
    format: ImageFormat,
) -> Result<DynamicImage> {
    let mut reader = ImageReader::with_format(Cursor::new(bytes), format);
    reader.limits(prompt_image_decode_limits());
    reader
        .decode()
        .with_context(|| format!("unsupported image at `{}`", path.display()))
}

fn prompt_image_probe_limits() -> Limits {
    let mut limits = Limits::default();
    limits.max_alloc = Some(MAX_IMAGE_DECODE_ALLOC_BYTES);
    limits
}

fn prompt_image_decode_limits() -> Limits {
    let mut limits = prompt_image_probe_limits();
    limits.max_image_width = Some(MAX_IMAGE_SOURCE_DIMENSION);
    limits.max_image_height = Some(MAX_IMAGE_SOURCE_DIMENSION);
    limits
}

fn encoded_from_bytes(mime: String, bytes: Vec<u8>, width: u32, height: u32) -> EncodedPromptImage {
    let base64_data = BASE64_STANDARD.encode(bytes);
    let data_url = format!("data:{mime};base64,{base64_data}");
    EncodedPromptImage {
        mime,
        base64_data,
        data_url,
        width,
        height,
    }
}

fn max_dimension_for_detail(detail: ImageDetail) -> u32 {
    match detail {
        ImageDetail::Low => LOW_IMAGE_DIMENSION,
        ImageDetail::Auto | ImageDetail::High | ImageDetail::Original => MAX_IMAGE_DIMENSION,
    }
}

fn can_preserve_source_bytes(format: ImageFormat) -> bool {
    matches!(
        format,
        ImageFormat::Png | ImageFormat::Jpeg | ImageFormat::WebP
    )
}

fn is_supported_format(format: ImageFormat) -> bool {
    matches!(
        format,
        ImageFormat::Png | ImageFormat::Jpeg | ImageFormat::WebP | ImageFormat::Gif
    )
}

fn mime_for_format(format: ImageFormat) -> &'static str {
    match format {
        ImageFormat::Png => "image/png",
        ImageFormat::Jpeg => "image/jpeg",
        ImageFormat::WebP => "image/webp",
        ImageFormat::Gif => "image/gif",
        _ => "application/octet-stream",
    }
}

fn hash_bytes(bytes: &[u8]) -> u64 {
    let mut hasher = DefaultHasher::new();
    bytes.hash(&mut hasher);
    hasher.finish()
}

#[cfg(test)]
mod tests {
    use super::*;
    use base64::Engine;
    use image::{ImageBuffer, ImageFormat, Rgba};
    use std::io::Cursor;
    use tempfile::tempdir;

    use crate::types::ImageDetail;

    fn png_bytes() -> Vec<u8> {
        let image = ImageBuffer::from_pixel(2, 2, Rgba([10u8, 20, 30, 255]));
        let mut bytes = Cursor::new(Vec::new());
        image::DynamicImage::ImageRgba8(image)
            .write_to(&mut bytes, ImageFormat::Png)
            .unwrap();
        bytes.into_inner()
    }

    fn oversized_png_bytes() -> Vec<u8> {
        let image = ImageBuffer::from_pixel(8193, 1, Rgba([10u8, 20, 30, 255]));
        let mut bytes = Cursor::new(Vec::new());
        image::DynamicImage::ImageRgba8(image)
            .write_to(&mut bytes, ImageFormat::Png)
            .unwrap();
        bytes.into_inner()
    }

    #[test]
    fn encodes_png_as_data_url() {
        let dir = tempdir().unwrap();
        let path = dir.path().join("image.png");
        std::fs::write(&path, png_bytes()).unwrap();

        let encoded = load_local_image_for_prompt(&path, ImageDetail::High).unwrap();

        assert_eq!(encoded.mime, "image/png");
        assert!(encoded.data_url.starts_with("data:image/png;base64,"));
        assert!(!encoded.base64_data.is_empty());
    }

    #[test]
    fn rejects_non_image_file() {
        let dir = tempdir().unwrap();
        let path = dir.path().join("not-image.json");
        std::fs::write(&path, br#"{"hello":"world"}"#).unwrap();

        let error = load_local_image_for_prompt(&path, ImageDetail::High).unwrap_err();

        assert!(error.to_string().contains("unsupported image"));
    }

    #[test]
    fn rejects_missing_image_file() {
        let dir = tempdir().unwrap();
        let path = dir.path().join("missing.png");

        let error = load_local_image_for_prompt(&path, ImageDetail::High).unwrap_err();

        assert!(error.to_string().contains("could not read image"));
    }

    #[test]
    fn rejects_source_bytes_over_limit_before_decode() {
        let path = std::path::Path::new("/tmp/oversized-source.png");
        let mut bytes = b"\x89PNG\r\n\x1a\n".to_vec();
        bytes.resize((20 * 1024 * 1024) + 1, 0);

        let error = encode_image_bytes_for_prompt(path, bytes, ImageDetail::High).unwrap_err();

        assert!(
            error
                .to_string()
                .contains("exceeds the 20971520 byte limit"),
            "{error:#}"
        );
    }

    #[test]
    fn rejects_source_dimensions_over_limit_before_decode() {
        let path = std::path::Path::new("/tmp/oversized-dimensions.png");

        let error = encode_image_bytes_for_prompt(path, oversized_png_bytes(), ImageDetail::High)
            .unwrap_err();

        assert!(error.to_string().contains("image dimensions"), "{error:#}");
    }

    #[test]
    fn png_within_limit_passes_through_without_reencode() {
        let dir = tempdir().unwrap();
        let path = dir.path().join("small.png");
        let bytes = png_bytes();
        std::fs::write(&path, &bytes).unwrap();

        let encoded = load_local_image_for_prompt(&path, ImageDetail::High).unwrap();

        assert_eq!(encoded.mime, "image/png");
        assert_eq!(
            encoded.base64_data,
            base64::engine::general_purpose::STANDARD.encode(&bytes)
        );
    }

    #[test]
    fn encoding_same_bytes_twice_is_idempotent() {
        let path = std::path::Path::new("/tmp/cache-probe.png");
        let bytes = png_bytes();

        let first = encode_image_bytes_for_prompt(path, bytes.clone(), ImageDetail::High).unwrap();
        let second = encode_image_bytes_for_prompt(path, bytes, ImageDetail::High).unwrap();

        assert_eq!(first, second);
    }
}
