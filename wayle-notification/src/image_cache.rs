use std::{
    borrow::Cow,
    collections::{HashSet, hash_map::DefaultHasher},
    fs,
    hash::{Hash, Hasher},
    io::BufWriter,
    path::{Path, PathBuf},
};

use png::ColorType;
use tracing::{debug, warn};

use crate::core::types::BorrowedImageData;

const EXPECTED_BITS_PER_SAMPLE: i32 = 8;
const RGB_CHANNELS: i32 = 3;
const RGBA_CHANNELS: i32 = 4;

/// Caches an already-encoded image blob (e.g. the PNG bytes from a GTK `bytes` icon, or the
/// bytes read from a portal `file-descriptor` icon) verbatim and returns the file path.
///
/// Unlike [`cache_borrowed_image`], `data` is not raw pixels — it is a self-contained
/// encoded image, so it is written as-is (the shell's `gtk::Image` sniffs the format).
pub(crate) fn cache_encoded_image(data: &[u8]) -> Option<String> {
    cache_blob(data, "img")
}

/// Caches a sound blob (the bytes read from a portal `file-descriptor` sound — ogg/opus,
/// ogg/vorbis or wav/pcm) verbatim and returns the file path, so the shell's future sound
/// service can play it from disk.
pub(crate) fn cache_encoded_sound(data: &[u8]) -> Option<String> {
    cache_blob(data, "snd")
}

/// Writes a self-contained blob verbatim to a content-addressed cache file with `extension`,
/// returning its path. Content-addressing dedupes identical blobs to a single file and makes
/// the write idempotent.
fn cache_blob(data: &[u8], extension: &str) -> Option<String> {
    if data.is_empty() {
        return None;
    }

    let dir = cache_dir();
    let path = dir.join(format!("{}.{extension}", content_hash(data)));

    if path.exists() {
        return Some(path_to_string(&path));
    }

    if let Err(err) = fs::create_dir_all(&dir) {
        warn!(error = %err, "cannot create image cache directory");
        return None;
    }

    // Write to a temp file then atomically rename, so a crash mid-write never leaves a truncated
    // file that `path.exists()` would later serve as a valid cache hit. The temp name is derived
    // from the same content hash: concurrent writers of identical content are harmless (they
    // write the same bytes), and different content never shares a temp path.
    let tmp = dir.join(format!("{}.{extension}.tmp", content_hash(data)));
    if let Err(err) = fs::write(&tmp, data) {
        warn!(error = %err, "cannot write cached blob");
        let _ = fs::remove_file(&tmp);
        return None;
    }
    if let Err(err) = fs::rename(&tmp, &path) {
        warn!(error = %err, "cannot finalize cached blob");
        let _ = fs::remove_file(&tmp);
        return None;
    }

    debug!(path = %path.display(), "cached notification blob");
    Some(path_to_string(&path))
}

/// Caches borrowed raw pixel data as a PNG file and returns the file path.
pub(crate) fn cache_borrowed_image(image: BorrowedImageData<'_>) -> Option<String> {
    cache_image_data(
        image.width,
        image.height,
        image.rowstride,
        image.bits_per_sample,
        image.channels,
        image.data,
    )
}

fn cache_image_data(
    width: i32,
    height: i32,
    rowstride: i32,
    bits_per_sample: i32,
    channels: i32,
    data: &[u8],
) -> Option<String> {
    let color_type = png_color_type(bits_per_sample, channels)?;

    let dir = cache_dir();
    let path = dir.join(format!("{}.png", content_hash(data)));

    if path.exists() {
        return Some(path_to_string(&path));
    }

    if let Err(err) = fs::create_dir_all(&dir) {
        warn!(error = %err, "cannot create image cache directory");
        return None;
    }

    let pixel_data = strip_rowstride_padding(width, channels, rowstride, data);
    encode_png(
        &path,
        width as u32,
        height as u32,
        color_type,
        pixel_data.as_ref(),
    )?;

    debug!(path = %path.display(), "cached notification image");
    Some(path_to_string(&path))
}

fn png_color_type(bits_per_sample: i32, channels: i32) -> Option<ColorType> {
    if bits_per_sample != EXPECTED_BITS_PER_SAMPLE {
        warn!(bits_per_sample, "unsupported bit depth, skipping PNG cache");
        return None;
    }

    match channels {
        RGB_CHANNELS => Some(ColorType::Rgb),
        RGBA_CHANNELS => Some(ColorType::Rgba),
        other => {
            warn!(
                channels = other,
                "unsupported channel count, skipping PNG cache"
            );
            None
        }
    }
}

fn strip_rowstride_padding<'a>(
    width: i32,
    channels: i32,
    rowstride: i32,
    data: &'a [u8],
) -> Cow<'a, [u8]> {
    let row_bytes = (channels * width) as usize;
    let rowstride = rowstride as usize;

    if rowstride == row_bytes {
        return Cow::Borrowed(data);
    }

    Cow::Owned(
        data.chunks(rowstride)
            .flat_map(|row| &row[..row_bytes.min(row.len())])
            .copied()
            .collect(),
    )
}

fn encode_png(
    path: &Path,
    width: u32,
    height: u32,
    color_type: ColorType,
    pixel_data: &[u8],
) -> Option<()> {
    // Encode into a temp file then atomically rename, so a crash mid-encode never leaves a
    // truncated PNG that `path.exists()` would later serve as a valid cache hit.
    let tmp = path.with_extension("png.tmp");
    let file = match fs::File::create(&tmp) {
        Ok(file) => file,
        Err(err) => {
            warn!(error = %err, "cannot create cached PNG file");
            return None;
        }
    };

    let mut encoder = png::Encoder::new(BufWriter::new(file), width, height);
    encoder.set_color(color_type);
    encoder.set_depth(png::BitDepth::Eight);
    encoder.set_compression(png::Compression::Fast);

    let mut writer = match encoder.write_header() {
        Ok(writer) => writer,
        Err(err) => {
            warn!(error = %err, "cannot write PNG header");
            let _ = fs::remove_file(&tmp);
            return None;
        }
    };

    if let Err(err) = writer.write_image_data(pixel_data) {
        warn!(error = %err, "cannot encode PNG pixel data");
        let _ = fs::remove_file(&tmp);
        return None;
    }

    // Finalize + flush the encoder before renaming so the temp file is complete on disk.
    if let Err(err) = writer.finish() {
        warn!(error = %err, "cannot finalize cached PNG");
        let _ = fs::remove_file(&tmp);
        return None;
    }

    if let Err(err) = fs::rename(&tmp, path) {
        warn!(error = %err, "cannot move cached PNG into place");
        let _ = fs::remove_file(&tmp);
        return None;
    }

    Some(())
}

/// Deletes cached blobs no longer referenced by any restored notification, called once at
/// startup to bound the otherwise-unbounded cache. Safe because the cache is content-
/// addressed: a file dropped here is simply re-created on demand if the same image/sound
/// reappears. `referenced` holds the cache paths still in use (from notification icons,
/// images and sound files); paths outside the cache directory are naturally ignored.
pub(crate) fn prune(referenced: &HashSet<PathBuf>) {
    let dir = cache_dir();
    let Ok(entries) = fs::read_dir(&dir) else {
        return;
    };

    let mut removed = 0usize;
    for entry in entries.flatten() {
        let path = entry.path();
        if !referenced.contains(&path) && fs::remove_file(&path).is_ok() {
            removed += 1;
        }
    }

    if removed > 0 {
        debug!(removed, "pruned unreferenced cached notification blobs");
    }
}

fn cache_dir() -> PathBuf {
    let base = std::env::var("XDG_CACHE_HOME")
        .or_else(|_| std::env::var("HOME").map(|home| format!("{home}/.cache")))
        .unwrap_or_else(|_| String::from("/tmp"));

    PathBuf::from(base).join("wayle/notifications")
}

fn content_hash(data: &[u8]) -> u64 {
    let mut hasher = DefaultHasher::new();
    data.hash(&mut hasher);
    hasher.finish()
}

fn path_to_string(path: &Path) -> String {
    path.to_string_lossy().into_owned()
}
