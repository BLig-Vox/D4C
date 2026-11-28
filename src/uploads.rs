use std::{io::Read, path::PathBuf, sync::mpsc, thread};
pub type Attachment = (String, String, Vec<u8>);
pub type UploadResult = Result<Vec<Attachment>, String>;
pub const FILE_LIMIT: usize = 6_000_000;
pub const TOTAL_LIMIT: usize = 12_000_000;

#[cfg(all(test, not(target_os = "android")))]
mod clipboard_tests {
    #[test]
    fn clipboard_rgba_becomes_a_png_attachment() {
        let file = super::clipboard_image_attachment(1, 1, vec![255, 0, 0, 255]).unwrap();
        assert_eq!(file.1, "image/png");
        let decoded = image::load_from_memory(&file.2).unwrap().to_rgba8();
        assert_eq!(decoded.get_pixel(0, 0).0, [255, 0, 0, 255]);
    }

    #[test]
    fn malformed_clipboard_pixels_are_rejected() {
        assert!(super::clipboard_image_attachment(2, 2, vec![0; 4]).is_err());
    }
}

pub fn load(paths: Vec<PathBuf>, ctx: eframe::egui::Context) -> mpsc::Receiver<UploadResult> {
    spawn(ctx, move || read_files(paths))
}
#[cfg(not(target_os = "android"))]
pub fn paste_image(
    image: std::sync::Arc<eframe::egui::ColorImage>,
    ctx: eframe::egui::Context,
) -> mpsc::Receiver<UploadResult> {
    spawn(ctx, move || {
        let [width, height] = image.size;
        if width.checked_mul(height).filter(|n| *n <= 32_000_000) != Some(image.pixels.len()) {
            return Err("Clipboard image is too large or invalid".into());
        }
        let pixels = image
            .pixels
            .iter()
            .flat_map(|p| p.to_srgba_unmultiplied())
            .collect();
        Ok(vec![clipboard_image_attachment(width, height, pixels)?])
    })
}
#[cfg(not(target_os = "android"))]
fn clipboard_image_attachment(
    width: usize,
    height: usize,
    pixels: Vec<u8>,
) -> Result<Attachment, String> {
    let width = u32::try_from(width).map_err(|_| "Invalid image dimensions")?;
    let height = u32::try_from(height).map_err(|_| "Invalid image dimensions")?;
    let image =
        image::RgbaImage::from_raw(width, height, pixels).ok_or("Invalid clipboard image")?;
    let mut png = std::io::Cursor::new(Vec::new());
    image::DynamicImage::ImageRgba8(image)
        .write_to(&mut png, image::ImageFormat::Png)
        .map_err(|e| format!("Could not attach clipboard image: {e}"))?;
    let bytes = png.into_inner();
    if bytes.len() > FILE_LIMIT {
        return Err("Attachment limit: 6 MB per file".into());
    }
    Ok(("Pasted image.png".into(), "image/png".into(), bytes))
}
#[cfg(target_os = "android")]
pub fn pick(ctx: eframe::egui::Context) -> mpsc::Receiver<UploadResult> {
    crate::android_uploads::pick(ctx)
}
#[cfg(not(target_os = "android"))]
pub fn pick(ctx: eframe::egui::Context) -> mpsc::Receiver<UploadResult> {
    spawn(ctx, || {
        let result = std::process::Command::new("zenity")
            .args([
                "--file-selection",
                "--multiple",
                "--separator=\n",
                "--title=Attach files",
                "--width=760",
                "--height=520",
            ])
            .output()
            .map_err(|_| {
                "File picker unavailable. Install zenity, or drop files into the chat.".to_owned()
            })?;
        if !result.status.success() {
            return Ok(vec![]);
        }
        read_files(
            String::from_utf8_lossy(&result.stdout)
                .lines()
                .map(PathBuf::from)
                .collect(),
        )
    })
}
fn spawn(
    ctx: eframe::egui::Context,
    work: impl FnOnce() -> UploadResult + Send + 'static,
) -> mpsc::Receiver<UploadResult> {
    let (tx, rx) = mpsc::channel();
    thread::spawn(move || {
        let _ = tx.send(work());
        ctx.request_repaint();
    });
    rx
}
fn read_files(paths: Vec<PathBuf>) -> UploadResult {
    let mut files = Vec::new();
    let mut total = 0;
    for path in paths {
        let name = path
            .file_name()
            .ok_or("Invalid file name")?
            .to_string_lossy()
            .to_string();
        let file = std::fs::File::open(&path).map_err(|e| format!("{name}: {e}"))?;
        let metadata = file.metadata().map_err(|e| e.to_string())?;
        if !metadata.is_file() || metadata.len() > FILE_LIMIT as u64 {
            return Err(format!("{name}: choose a regular file under 6 MB"));
        }
        let mut bytes = Vec::new();
        file.take(FILE_LIMIT as u64 + 1)
            .read_to_end(&mut bytes)
            .map_err(|e| format!("{name}: {e}"))?;
        total += bytes.len();
        if bytes.len() > FILE_LIMIT || total > TOTAL_LIMIT {
            return Err("Attachment limit: 6 MB per file, 12 MB total".into());
        }
        files.push((name, super::mime_for(&path).into(), bytes));
    }
    Ok(files)
}
