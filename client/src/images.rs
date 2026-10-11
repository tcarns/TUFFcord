//! Image attachments on the app side: getting an image (file picker, drag and
//! drop, clipboard), shrinking it before upload, and decoding received images
//! at the size they're shown.

use eframe::egui::ColorImage;
use image::imageops::FilterType;
use image::{DynamicImage, GenericImageView, ImageFormat};
use std::path::{Path, PathBuf};

/// Longest side after shrinking. Plenty for a chat image, and keeps uploads small.
pub const MAX_SIDE: u32 = 2048;
/// Images already this small (and not huge in pixels) are sent untouched.
pub const KEEP_ORIGINAL_BYTES: usize = 1_500_000;
/// Thumbnail size for the "about to send" preview.
pub const PREVIEW_SIDE: u32 = 160;

pub struct Prepared {
    pub bytes: Vec<u8>,
    pub mime: &'static str,
    pub name: String,
    pub width: u32,
    pub height: u32,
    pub preview: ColorImage,
}

fn mime_of(f: ImageFormat) -> &'static str {
    match f {
        ImageFormat::Png => "image/png",
        ImageFormat::Jpeg => "image/jpeg",
        ImageFormat::Gif => "image/gif",
        ImageFormat::WebP => "image/webp",
        _ => "application/octet-stream",
    }
}

fn ext_of(mime: &str) -> &'static str {
    match mime {
        "image/png" => "png",
        "image/jpeg" => "jpg",
        "image/gif" => "gif",
        "image/webp" => "webp",
        _ => "img",
    }
}

/// File name with the extension matching what we actually send.
fn renamed(name: &str, mime: &str) -> String {
    let stem = Path::new(name)
        .file_stem()
        .and_then(|s| s.to_str())
        .filter(|s| !s.trim().is_empty())
        .unwrap_or("image");
    let stem: String = stem.chars().take(70).collect();
    format!("{stem}.{}", ext_of(mime))
}

pub fn to_color_image(img: &DynamicImage) -> ColorImage {
    let rgba = img.to_rgba8();
    ColorImage::from_rgba_unmultiplied(
        [rgba.width() as usize, rgba.height() as usize],
        rgba.as_raw(),
    )
}

fn has_transparency(img: &DynamicImage) -> bool {
    img.color().has_alpha() && img.to_rgba8().pixels().any(|p| p.0[3] < 250)
}

/// Turn a file's bytes into something ready to send: decoded to check it's a
/// real image, shrunk if large, re-encoded if needed (JPEG, or PNG when it has
/// transparency). GIFs are kept as-is when they fit, so animations survive.
pub fn prepare(bytes: Vec<u8>, name: &str, max_bytes: u64) -> Result<Prepared, String> {
    let format = image::guess_format(&bytes).map_err(|_| {
        "That file isn't an image TUFFcord can send (PNG, JPEG, GIF, WebP or BMP).".to_string()
    })?;
    let img = image::load_from_memory_with_format(&bytes, format)
        .map_err(|e| format!("Couldn't read that image ({e})."))?;
    let (w, h) = img.dimensions();
    let web_format = matches!(
        format,
        ImageFormat::Png | ImageFormat::Jpeg | ImageFormat::Gif | ImageFormat::WebP
    );
    let small_enough = bytes.len() <= KEEP_ORIGINAL_BYTES && w.max(h) <= 2560;
    let keep_gif = format == ImageFormat::Gif && bytes.len() as u64 <= max_bytes;

    let (out, mime, ow, oh) = if (web_format && small_enough) || keep_gif {
        (bytes, mime_of(format), w, h)
    } else {
        let scaled = if w.max(h) > MAX_SIDE {
            img.resize(MAX_SIDE, MAX_SIDE, FilterType::Triangle)
        } else {
            img.clone()
        };
        let (sw, sh) = scaled.dimensions();
        let mut out = Vec::new();
        let mime = if has_transparency(&scaled) {
            scaled
                .write_to(&mut std::io::Cursor::new(&mut out), ImageFormat::Png)
                .map_err(|e| format!("Couldn't shrink that image ({e})."))?;
            "image/png"
        } else {
            let rgb = scaled.to_rgb8();
            image::codecs::jpeg::JpegEncoder::new_with_quality(&mut out, 85)
                .encode_image(&rgb)
                .map_err(|e| format!("Couldn't shrink that image ({e})."))?;
            "image/jpeg"
        };
        (out, mime, sw, sh)
    };
    if out.len() as u64 > max_bytes {
        return Err(format!(
            "That image is too big to send ({} MB max).",
            max_bytes / 1_048_576
        ));
    }
    Ok(Prepared {
        preview: to_color_image(&img.thumbnail(PREVIEW_SIDE, PREVIEW_SIDE)),
        name: renamed(name, mime),
        bytes: out,
        mime,
        width: ow,
        height: oh,
    })
}

pub fn prepare_file(path: &Path, max_bytes: u64) -> Result<Prepared, String> {
    let meta = std::fs::metadata(path).map_err(|e| format!("Couldn't open that file ({e})."))?;
    if meta.len() > 60 * 1024 * 1024 {
        return Err("That file is too big to send.".into());
    }
    let bytes = std::fs::read(path).map_err(|e| format!("Couldn't open that file ({e})."))?;
    let name = path.file_name().and_then(|n| n.to_str()).unwrap_or("image");
    prepare(bytes, name, max_bytes)
}

/// Limits for decoding images other people sent: a tiny file can claim to be
/// huge, and the decoder would set aside memory for all of it.
fn received_limits() -> image::Limits {
    let mut l = image::Limits::default();
    l.max_image_width = Some(8192);
    l.max_image_height = Some(8192);
    l.max_alloc = Some(160 * 1024 * 1024);
    l
}

/// Decode an image to fit within max_w by max_h pixels (never upscaled).
pub fn decode_fit(bytes: &[u8], max_w: u32, max_h: u32) -> Result<ColorImage, String> {
    let mut reader = image::ImageReader::new(std::io::Cursor::new(bytes))
        .with_guessed_format()
        .map_err(|e| e.to_string())?;
    reader.limits(received_limits());
    let img = reader.decode().map_err(|e| e.to_string())?;
    let img = if img.width() > max_w || img.height() > max_h {
        img.resize(max_w.max(1), max_h.max(1), FilterType::Triangle)
    } else {
        img
    };
    Ok(to_color_image(&img))
}

/// An image on the clipboard (e.g. after Win+Shift+S or "Copy image"), as PNG bytes.
/// None when the clipboard holds no image.
pub fn clipboard_image() -> Option<Result<Vec<u8>, String>> {
    let mut cb = arboard::Clipboard::new().ok()?;
    let img = cb.get_image().ok()?;
    let buf =
        image::RgbaImage::from_raw(img.width as u32, img.height as u32, img.bytes.into_owned())?;
    let mut out = Vec::new();
    let written = DynamicImage::ImageRgba8(buf)
        .write_to(&mut std::io::Cursor::new(&mut out), ImageFormat::Png);
    Some(match written {
        Ok(()) => Ok(out),
        Err(e) => Err(format!("Couldn't read the pasted image ({e}).")),
    })
}

/// Size label for people: "240 KB", "1.4 MB".
pub fn size_label(bytes: u64) -> String {
    proto::files::size_label(bytes)
}

/// The image type of some bytes, for the older (WebSocket) way of sending.
pub fn mime_for_name(bytes: &[u8]) -> &'static str {
    proto::sniff_image(bytes).unwrap_or("image/png")
}

/// Decode a GIF's frames to fit within max_w by max_h, stopping once they'd
/// take more than `budget` bytes. Each frame comes with how long it shows (ms).
pub fn decode_gif(
    path: &Path,
    max_w: u32,
    max_h: u32,
    budget: usize,
) -> Result<Vec<(ColorImage, u32)>, String> {
    use image::{AnimationDecoder, ImageDecoder};
    let file = std::fs::File::open(path).map_err(|e| e.to_string())?;
    let mut decoder = image::codecs::gif::GifDecoder::new(std::io::BufReader::new(file))
        .map_err(|e| e.to_string())?;
    decoder
        .set_limits(received_limits())
        .map_err(|e| e.to_string())?;
    let mut out = Vec::new();
    let mut used = 0usize;
    for frame in decoder.into_frames() {
        let frame = match frame {
            Ok(f) => f,
            Err(_) if !out.is_empty() => break, // keep what decoded
            Err(e) => return Err(e.to_string()),
        };
        let (num, den) = frame.delay().numer_denom_ms();
        let ms = if den == 0 { 100 } else { num / den };
        // Like browsers: very short delays mean "as fast as allowed", shown at 100 ms.
        let ms = if ms < 20 { 100 } else { ms };
        let buf = frame.into_buffer();
        let img = if buf.width() > max_w || buf.height() > max_h {
            DynamicImage::ImageRgba8(buf).resize(max_w.max(1), max_h.max(1), FilterType::Triangle)
        } else {
            DynamicImage::ImageRgba8(buf)
        };
        used += (img.width() * img.height() * 4) as usize;
        if used > budget && !out.is_empty() {
            break;
        }
        out.push((to_color_image(&img), ms));
    }
    Ok(out)
}

/// Save an image to a temporary file and open it in the default photo viewer
/// (older servers, where the bytes came over the WebSocket).
pub fn open_externally(id: &str, name: &str, bytes: &[u8]) -> Result<(), String> {
    let dir = std::env::temp_dir().join("TUFFcord");
    std::fs::create_dir_all(&dir).map_err(|e| e.to_string())?;
    let safe: String = name
        .chars()
        .map(|c| {
            if c.is_alphanumeric() || c == '.' || c == '-' || c == '_' {
                c
            } else {
                '_'
            }
        })
        .collect();
    // The extension comes from what the bytes are, never the sender's name
    // (an image named x.hta would open in a script host).
    let ext = match proto::sniff_image(bytes) {
        Some("image/jpeg") => "jpg",
        Some("image/gif") => "gif",
        Some("image/webp") => "webp",
        Some(_) => "png",
        None => return Err("that isn't an image".into()),
    };
    let stem = safe.rsplit_once('.').map_or(safe.as_str(), |(s, _)| s);
    let short: String = id.chars().take(8).collect();
    let path = dir.join(format!("{short}-{stem}.{ext}"));
    std::fs::write(&path, bytes).map_err(|e| e.to_string())?;
    crate::files::mark_downloaded(&path);
    #[cfg(windows)]
    let result = std::process::Command::new("explorer").arg(&path).spawn();
    #[cfg(not(windows))]
    let result = std::process::Command::new("xdg-open").arg(&path).spawn();
    result.map(|_| ()).map_err(|e| e.to_string())
}

/// The Windows "Open" dialog. Blocks until closed, so call it off the UI thread.
/// `any`: any file (servers from 0.7); otherwise images only.
#[cfg(windows)]
pub fn pick_file(any: bool) -> Option<PathBuf> {
    use windows_sys::Win32::System::Com::{CoInitializeEx, COINIT_APARTMENTTHREADED};
    use windows_sys::Win32::UI::Controls::Dialogs::{
        GetOpenFileNameW, OFN_EXPLORER, OFN_FILEMUSTEXIST, OFN_NOCHANGEDIR, OFN_PATHMUSTEXIST,
        OPENFILENAMEW,
    };
    let wide = |s: &str| s.encode_utf16().collect::<Vec<u16>>();
    let images = "Images (PNG, JPEG, GIF, WebP, BMP)\0*.png;*.jpg;*.jpeg;*.gif;*.webp;*.bmp\0";
    let filter = if any {
        wide(&format!("All files\0*.*\0{images}Videos\0*.mp4;*.mov;*.webm;*.mkv;*.avi;*.m4v;*.wmv\0Audio\0*.mp3;*.wav;*.ogg;*.flac;*.m4a;*.aac;*.opus\0\0"))
    } else {
        wide(&format!("{images}\0"))
    };
    let title = wide(if any {
        "Send a file\0"
    } else {
        "Send an image\0"
    });
    let mut buf = vec![0u16; 4096];
    unsafe {
        CoInitializeEx(std::ptr::null(), COINIT_APARTMENTTHREADED as u32);
        let mut ofn: OPENFILENAMEW = std::mem::zeroed();
        ofn.lStructSize = std::mem::size_of::<OPENFILENAMEW>() as u32;
        ofn.lpstrFilter = filter.as_ptr();
        ofn.lpstrFile = buf.as_mut_ptr();
        ofn.nMaxFile = buf.len() as u32;
        ofn.lpstrTitle = title.as_ptr();
        ofn.Flags = OFN_FILEMUSTEXIST | OFN_PATHMUSTEXIST | OFN_EXPLORER | OFN_NOCHANGEDIR;
        if GetOpenFileNameW(&mut ofn) == 0 {
            return None;
        }
    }
    let len = buf.iter().position(|&c| c == 0).unwrap_or(0);
    Some(PathBuf::from(String::from_utf16_lossy(&buf[..len])))
}

#[cfg(not(windows))]
pub fn pick_file(_any: bool) -> Option<PathBuf> {
    None
}

/// The Windows "Save as" dialog, starting with `name`. Blocks; call it off the UI thread.
#[cfg(windows)]
pub fn save_file_dialog(name: &str) -> Option<PathBuf> {
    use windows_sys::Win32::System::Com::{CoInitializeEx, COINIT_APARTMENTTHREADED};
    use windows_sys::Win32::UI::Controls::Dialogs::{
        GetSaveFileNameW, OFN_EXPLORER, OFN_NOCHANGEDIR, OFN_OVERWRITEPROMPT, OFN_PATHMUSTEXIST,
        OPENFILENAMEW,
    };
    let wide = |s: &str| s.encode_utf16().collect::<Vec<u16>>();
    let filter = wide("All files\0*.*\0\0");
    let title = wide("Save as\0");
    let mut buf = vec![0u16; 4096];
    for (i, c) in proto::files::clean_name(name)
        .encode_utf16()
        .take(4000)
        .enumerate()
    {
        buf[i] = c;
    }
    unsafe {
        CoInitializeEx(std::ptr::null(), COINIT_APARTMENTTHREADED as u32);
        let mut ofn: OPENFILENAMEW = std::mem::zeroed();
        ofn.lStructSize = std::mem::size_of::<OPENFILENAMEW>() as u32;
        ofn.lpstrFilter = filter.as_ptr();
        ofn.lpstrFile = buf.as_mut_ptr();
        ofn.nMaxFile = buf.len() as u32;
        ofn.lpstrTitle = title.as_ptr();
        ofn.Flags = OFN_OVERWRITEPROMPT | OFN_PATHMUSTEXIST | OFN_EXPLORER | OFN_NOCHANGEDIR;
        if GetSaveFileNameW(&mut ofn) == 0 {
            return None;
        }
    }
    let len = buf.iter().position(|&c| c == 0).unwrap_or(0);
    Some(PathBuf::from(String::from_utf16_lossy(&buf[..len])))
}

/// Elsewhere (testing): saves into the Downloads folder.
#[cfg(not(windows))]
pub fn save_file_dialog(name: &str) -> Option<PathBuf> {
    dirs::download_dir().map(|d| d.join(proto::files::clean_name(name)))
}

pub fn has_file_picker() -> bool {
    cfg!(windows)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn png(w: u32, h: u32, alpha: bool) -> Vec<u8> {
        let img = image::RgbaImage::from_fn(w, h, |x, y| {
            image::Rgba([
                (x % 256) as u8,
                (y % 256) as u8,
                ((x * y) % 256) as u8,
                if alpha && x < w / 2 { 100 } else { 255 },
            ])
        });
        let mut out = Vec::new();
        DynamicImage::ImageRgba8(img)
            .write_to(&mut std::io::Cursor::new(&mut out), ImageFormat::Png)
            .unwrap();
        out
    }

    #[test]
    fn small_images_are_sent_untouched() {
        let bytes = png(300, 200, false);
        let p = prepare(bytes.clone(), "cat.png", 8 << 20).unwrap();
        assert_eq!(p.bytes, bytes);
        assert_eq!(
            (p.mime, p.width, p.height, p.name.as_str()),
            ("image/png", 300, 200, "cat.png")
        );
        assert!(p.preview.size[0] <= PREVIEW_SIDE as usize);
    }

    #[test]
    fn big_photos_are_shrunk_to_jpeg() {
        let p = prepare(png(3000, 2000, false), "huge photo.png", 8 << 20).unwrap();
        assert_eq!((p.mime, p.width, p.height), ("image/jpeg", 2048, 1365));
        assert_eq!(p.name, "huge photo.jpg");
        assert!(p.bytes.len() < 3_000_000, "{}", p.bytes.len());
        assert_eq!(&p.bytes[..3], &[0xFF, 0xD8, 0xFF]);
    }

    #[test]
    fn transparency_is_kept_as_png() {
        let p = prepare(png(3000, 2000, true), "logo.png", 8 << 20).unwrap();
        assert_eq!(p.mime, "image/png");
        assert_eq!(p.width, 2048);
    }

    #[test]
    fn rejects_non_images_and_oversize() {
        assert!(prepare(b"MZ this is an exe".to_vec(), "x.exe", 8 << 20).is_err());
        assert!(prepare(png(3000, 2000, true), "a.png", 50_000).is_err());
    }

    #[test]
    fn gif_frames_and_budget() {
        use image::codecs::gif::{GifEncoder, Repeat};
        use image::{Delay, Frame};
        let dir = std::env::temp_dir().join(format!("br-gif-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        let path = dir.join("a.gif");
        {
            let f = std::fs::File::create(&path).unwrap();
            let mut enc = GifEncoder::new(f);
            enc.set_repeat(Repeat::Infinite).unwrap();
            for i in 0..5u8 {
                let img = image::RgbaImage::from_pixel(200, 100, image::Rgba([i * 40, 0, 0, 255]));
                enc.encode_frame(Frame::from_parts(
                    img,
                    0,
                    0,
                    Delay::from_numer_denom_ms(50, 1),
                ))
                .unwrap();
            }
        }
        let frames = decode_gif(&path, 100, 100, 1 << 30).unwrap();
        assert_eq!(frames.len(), 5);
        assert_eq!(frames[0].0.size, [100, 50]);
        assert_eq!(frames[0].1, 50);
        // A tight budget keeps the first frames only.
        let few = decode_gif(&path, 100, 100, 100 * 50 * 4 * 2).unwrap();
        assert_eq!(few.len(), 2);
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn decode_fit_never_upscales() {
        let bytes = png(100, 50, false);
        assert_eq!(decode_fit(&bytes, 400, 300).unwrap().size, [100, 50]);
        assert_eq!(decode_fit(&bytes, 50, 50).unwrap().size, [50, 25]);
        // A small file claiming to be huge isn't decoded.
        assert!(decode_fit(&png(9000, 2, false), 400, 300).is_err());
    }
}
