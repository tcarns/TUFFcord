//! Profile pictures without UI: shrinking a chosen image to the small square
//! PNG the server takes, sending or removing yours, fetching others' into the
//! cache on disk. Everything here blocks, so it runs on worker threads.

use crate::files::{agent, explain, user_agent, Endpoint};
use image::imageops::FilterType;
use image::{DynamicImage, ImageFormat};
use proto::files::{AVATAR_SIDE, MAX_AVATAR};
use std::path::{Path, PathBuf};

/// Where fetched pictures are kept: %LOCALAPPDATA%\TUFFcord\avatars, one file
/// per picture hash (so a changed picture is a new file).
pub fn cache_path(hash: &str) -> PathBuf {
    let safe: String = hash
        .chars()
        .filter(|c| c.is_ascii_hexdigit())
        .take(64)
        .collect();
    dirs::cache_dir()
        .unwrap_or_else(std::env::temp_dir)
        .join("TUFFcord")
        .join("avatars")
        .join(format!("{safe}.png"))
}

/// The middle square of an image, shrunk to the picture size, as PNG bytes.
pub fn make(path: &Path) -> Result<Vec<u8>, String> {
    let bytes = std::fs::read(path).map_err(|e| format!("Couldn't read the file ({e})."))?;
    let mut reader = image::ImageReader::new(std::io::Cursor::new(bytes))
        .with_guessed_format()
        .map_err(|e| e.to_string())?;
    reader.limits(crate::images::received_limits());
    let img = reader
        .decode()
        .map_err(|_| "That isn't a picture TUFFcord can read.".to_string())?;
    let side = img.width().min(img.height()).max(1);
    let img = img
        .crop_imm(
            (img.width() - side) / 2,
            (img.height() - side) / 2,
            side,
            side,
        )
        .resize_exact(AVATAR_SIDE, AVATAR_SIDE, FilterType::Lanczos3);
    let png = |img: &DynamicImage| {
        let mut out = std::io::Cursor::new(Vec::new());
        img.write_to(&mut out, ImageFormat::Png)
            .map(|_| out.into_inner())
            .map_err(|e| e.to_string())
    };
    let mut out = png(&DynamicImage::ImageRgba8(img.to_rgba8()))?;
    if out.len() > MAX_AVATAR {
        // A very busy picture: without transparency it always fits.
        out = png(&DynamicImage::ImageRgb8(img.to_rgb8()))?;
    }
    Ok(out)
}

/// Set your picture (`Some`) or remove it (`None`).
pub fn send(to: &Endpoint, png: Option<&[u8]>) -> Result<(), String> {
    let url = format!("{}/avatar", to.base);
    let auth = format!("Bearer {}", to.key);
    let agent = agent();
    let resp = match png {
        Some(bytes) => agent
            .put(&url)
            .header("Authorization", &auth)
            .header("User-Agent", &user_agent())
            .header("Content-Type", "image/png")
            .send(bytes),
        None => agent
            .delete(&url)
            .header("Authorization", &auth)
            .header("User-Agent", &user_agent())
            .call(),
    };
    let mut resp = resp.map_err(|e| format!("Couldn't reach the server ({e})."))?;
    match resp.status().as_u16() {
        200 => Ok(()),
        s => Err(explain(
            s,
            &resp.body_mut().read_to_string().unwrap_or_default(),
        )),
    }
}

/// Fetch `account`'s picture into the cache (unless it's there already).
pub fn fetch(from: &Endpoint, account: u32, hash: &str) -> Result<PathBuf, String> {
    let dest = cache_path(hash);
    if dest.exists() {
        return Ok(dest);
    }
    let mut resp = agent()
        .get(&format!("{}/avatars/{account}", from.base))
        .header("Authorization", &format!("Bearer {}", from.key))
        .header("User-Agent", &user_agent())
        .call()
        .map_err(|e| e.to_string())?;
    if resp.status().as_u16() != 200 {
        return Err(format!("the server answered {}", resp.status()));
    }
    let bytes = resp
        .body_mut()
        .with_config()
        .limit(MAX_AVATAR as u64)
        .read_to_vec()
        .map_err(|e| e.to_string())?;
    if let Some(dir) = dest.parent() {
        std::fs::create_dir_all(dir).map_err(|e| e.to_string())?;
    }
    std::fs::write(&dest, bytes).map_err(|e| e.to_string())?;
    Ok(dest)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn makes_small_squares() {
        let dir = std::env::temp_dir().join(format!("tuffcord-avatar-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        // A wide, noisy picture: the worst case for size.
        let mut img = image::RgbaImage::new(300, 200);
        let mut x: u32 = 1;
        for p in img.pixels_mut() {
            x = x.wrapping_mul(1_664_525).wrapping_add(1_013_904_223);
            *p = image::Rgba(x.to_le_bytes());
        }
        let path = dir.join("noise.png");
        img.save(&path).unwrap();
        let png = make(&path).unwrap();
        assert!(png.len() <= MAX_AVATAR, "{} bytes", png.len());
        let back = image::load_from_memory(&png).unwrap();
        assert_eq!((back.width(), back.height()), (AVATAR_SIDE, AVATAR_SIDE));
        let _ = std::fs::remove_dir_all(dir);
    }
}
