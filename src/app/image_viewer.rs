//! Full-screen, read-only image inspection. One worker owns the original/vector;
//! only the visible viewport is rasterized and encoded, never a giant zoomed bitmap.
use super::App;
use anyhow::{Context, Result, bail};
use crossterm::event::{KeyCode, KeyEvent, KeyModifiers, MouseButton, MouseEvent, MouseEventKind};
use image::{DynamicImage, GenericImage, imageops::FilterType};
use ratatui::{
    Frame,
    layout::{Rect, Size},
    style::Style,
    widgets::{Block, Paragraph, Wrap},
};
use ratatui_image::{
    picker::Picker,
    sliced::{SlicedImage, SlicedProtocol},
};
use std::{
    fs,
    path::PathBuf,
    sync::{
        Arc,
        atomic::{AtomicBool, Ordering},
        mpsc,
    },
};

#[derive(Clone, Copy, Debug, PartialEq)]
struct View {
    cells: Size,
    pixels: (u32, u32),
    zoom: f64,
    center: (f64, f64),
    generation: u64,
}

/// Destination coordinates in viewport pixels, clamped so panning cannot lose the image.
fn geometry(size: (f64, f64), view: &View) -> (f64, f64, f64) {
    let (w, h) = (f64::from(view.pixels.0), f64::from(view.pixels.1));
    let scale = (w / size.0).min(h / size.1) * view.zoom;
    let position = |viewport: f64, image: f64, center: f64| {
        if image <= viewport {
            (viewport - image) / 2.0
        } else {
            (viewport / 2.0 - center * image).clamp(viewport - image, 0.0)
        }
    };
    (
        position(w, size.0 * scale, view.center.0),
        position(h, size.1 * scale, view.center.1),
        scale,
    )
}

struct Export {
    path: PathBuf,
    // Keep generated files alive until application exit, including after the modal closes.
    directory: Option<Arc<tempfile::TempDir>>,
}

enum Update {
    Loaded((f64, f64), Export),
    Frame(u64, Result<SlicedProtocol, String>),
    Failed(String),
}

enum Source {
    Raster(DynamicImage),
    Vector(crate::mermaid::Vector),
}

impl Source {
    fn size(&self) -> (f64, f64) {
        match self {
            Self::Raster(image) => (image.width().into(), image.height().into()),
            Self::Vector(vector) => (vector.width, vector.height),
        }
    }
    fn viewport(&self, view: &View, cancel: &AtomicBool) -> Result<DynamicImage> {
        let (x, y, scale) = geometry(self.size(), view);
        let (width, height) = view.pixels;
        match self {
            Self::Vector(vector) => crate::mermaid::render_viewport(
                vector,
                serde_json::json!({
                    "width": width, "height":height, "x":x, "y":y, "scale":scale
                }),
                cancel,
            ),
            Self::Raster(original) => {
                let sx = (-x / scale).max(0.0).floor() as u32;
                let sy = (-y / scale).max(0.0).floor() as u32;
                let sw = ((f64::from(width) - x) / scale)
                    .ceil()
                    .min(f64::from(original.width())) as u32
                    - sx;
                let sh = ((f64::from(height) - y) / scale)
                    .ceil()
                    .min(f64::from(original.height())) as u32
                    - sy;
                let dx = x.max(0.0).round() as u32;
                let dy = y.max(0.0).round() as u32;
                let dw = (f64::from(sw) * scale).round().max(1.0) as u32;
                let dh = (f64::from(sh) * scale).round().max(1.0) as u32;
                let crop = original.crop_imm(sx, sy, sw.max(1), sh.max(1));
                let resized = crop.resize_exact(
                    dw.min(width - dx),
                    dh.min(height - dy),
                    FilterType::Triangle,
                );
                let mut canvas = DynamicImage::new_rgba8(width, height);
                canvas.copy_from(&resized, dx, dy)?;
                Ok(canvas)
            }
        }
    }
}

fn load(
    note: PathBuf,
    url: &str,
    settings: &crate::settings::Settings,
    cancel: &AtomicBool,
) -> Result<(Source, Export)> {
    if let Some(source) = crate::mermaid::source(url) {
        let vector = crate::mermaid::render_vector(source, settings, cancel)?;
        let directory = Arc::new(tempfile::tempdir()?);
        // A local wrapper keeps the SVG vector-sharp while its CSP continues to
        // block diagram-supplied network resources in the user's browser.
        let path = directory.path().join("diagram.html");
        fs::write(
            &path,
            format!(
                "<!doctype html><html><head><meta charset=\"utf-8\"><meta http-equiv=\"Content-Security-Policy\" content=\"default-src 'none'; style-src 'unsafe-inline'; img-src data:; base-uri 'none'; form-action 'none'\"><title>LinkNote — Mermaid</title><style>body{{margin:16px;background:{}}}svg{{max-width:none}}</style></head><body>{}</body></html>",
                vector.background, vector.svg
            ),
        )?;
        Ok((
            Source::Vector(vector),
            Export {
                path,
                directory: Some(directory),
            },
        ))
    } else {
        let image = crate::images::load_full_image(&note, url)?;
        let export = if crate::images::is_remote(url) {
            let directory = Arc::new(tempfile::tempdir()?);
            let path = directory.path().join("image.png");
            image.save(&path)?;
            Export {
                path,
                directory: Some(directory),
            }
        } else {
            let path = percent_encoding::percent_decode_str(url).decode_utf8()?;
            Export {
                path: note
                    .parent()
                    .context("笔记目录不存在")?
                    .join(path.as_ref())
                    .canonicalize()?,
                directory: None,
            }
        };
        Ok((Source::Raster(image), export))
    }
}

pub(super) struct Viewer {
    title: String,
    picker: Picker,
    tx: mpsc::Sender<View>,
    rx: mpsc::Receiver<Update>,
    cancel: Arc<AtomicBool>,
    export: Option<Export>,
    size: Option<(f64, f64)>,
    protocol: Option<SlicedProtocol>,
    error: Option<String>,
    notice: Option<String>,
    view: View,
    sent: Option<View>,
    busy: bool,
    rect: Rect,
    close_button: Rect,
    open_button: Rect,
    drag: Option<(u16, u16)>,
}

impl Drop for Viewer {
    fn drop(&mut self) {
        self.cancel.store(true, Ordering::Relaxed);
    }
}

impl Viewer {
    fn new(
        note: PathBuf,
        url: String,
        picker: Picker,
        settings: crate::settings::Settings,
    ) -> Self {
        let (tx, commands) = mpsc::channel::<View>();
        let (updates, rx) = mpsc::channel();
        let cancel = Arc::new(AtomicBool::new(false));
        let stopped = cancel.clone();
        let worker_picker = picker.clone();
        let title = if crate::mermaid::source(&url).is_some() {
            "Mermaid".into()
        } else {
            url.chars().filter(|c| !c.is_control()).take(100).collect()
        };
        std::thread::spawn(move || {
            let (source, export) = match load(note, &url, &settings, &stopped) {
                Ok(loaded) => loaded,
                Err(e) => {
                    let _ = updates.send(Update::Failed(format!("{e:#}")));
                    return;
                }
            };
            if stopped.load(Ordering::Relaxed)
                || updates.send(Update::Loaded(source.size(), export)).is_err()
            {
                return;
            }
            while let Ok(mut view) = commands.recv() {
                // Rapid key presses coalesce; stale generations are never displayed.
                while let Ok(latest) = commands.try_recv() {
                    view = latest;
                }
                if stopped.load(Ordering::Relaxed) {
                    break;
                }
                let result = source
                    .viewport(&view, &stopped)
                    .and_then(|image| {
                        Ok(SlicedProtocol::new(
                            &worker_picker,
                            image,
                            Some(view.cells),
                        )?)
                    })
                    .map_err(|e| format!("{e:#}"));
                if updates
                    .send(Update::Frame(view.generation, result))
                    .is_err()
                {
                    break;
                }
            }
        });
        Self {
            title,
            picker,
            tx,
            rx,
            cancel,
            export: None,
            size: None,
            protocol: None,
            error: None,
            notice: None,
            view: View {
                cells: Size::new(0, 0),
                pixels: (0, 0),
                zoom: 1.0,
                center: (0.5, 0.5),
                generation: 0,
            },
            sent: None,
            busy: true,
            rect: Rect::default(),
            close_button: Rect::default(),
            open_button: Rect::default(),
            drag: None,
        }
    }
    pub(super) fn poll(&mut self) {
        while let Ok(update) = self.rx.try_recv() {
            match update {
                Update::Loaded(size, export) => {
                    self.size = Some(size);
                    self.export = Some(export);
                }
                Update::Frame(generation, result) if generation == self.view.generation => {
                    self.busy = false;
                    match result {
                        Ok(protocol) => {
                            self.protocol = Some(protocol);
                            self.error = None;
                        }
                        Err(error) => self.error = Some(error),
                    }
                }
                Update::Failed(error) => {
                    self.error = Some(error);
                    self.busy = false;
                }
                _ => {}
            }
        }
    }
    fn changed(&mut self) {
        self.view.generation += 1;
        self.busy = self.size.is_some() || self.error.is_none();
        self.notice = None;
    }
    fn zoom(&mut self, factor: f64) {
        self.view.zoom = (self.view.zoom * factor).clamp(0.25, 16.0);
        self.changed();
    }
    fn pan(&mut self, dx: f64, dy: f64) {
        let Some(size) = self.size else {
            return;
        };
        if self.view.pixels.0 == 0 || self.view.pixels.1 == 0 {
            return;
        }
        let (x, y, scale) = geometry(size, &self.view);
        let (w, h) = (f64::from(self.view.pixels.0), f64::from(self.view.pixels.1));
        // Derive the actual clamped center first, avoiding dead travel at the edges.
        self.view.center = (
            ((w / 2.0 - x + dx) / (size.0 * scale)).clamp(0.0, 1.0),
            ((h / 2.0 - y + dy) / (size.1 * scale)).clamp(0.0, 1.0),
        );
        self.changed();
    }
    pub(super) fn draw(&mut self, f: &mut Frame, theme: crate::settings::Theme) {
        self.poll();
        let area = f.area();
        let palette = theme.palette();
        f.render_widget(
            Block::default().style(Style::default().bg(palette.bg).fg(palette.fg)),
            area,
        );
        self.rect = Rect::new(
            area.x,
            area.y.saturating_add(1),
            area.width,
            area.height.saturating_sub(3),
        );
        self.close_button = Rect::new(
            area.right().saturating_sub(12).max(area.x),
            area.y,
            area.width.min(12),
            1,
        );
        self.open_button = Rect::new(
            area.x,
            area.bottom().saturating_sub(1),
            area.width.min(18),
            1,
        );
        if self.rect.width > 0 && self.rect.height > 0 {
            let font = self.picker.font_size();
            let cells = self.rect.as_size();
            if self.view.cells != cells {
                self.view.cells = cells;
                let w = u32::from(cells.width) * u32::from(font.width);
                let h = u32::from(cells.height) * u32::from(font.height);
                let limit = (4096.0 / f64::from(w))
                    .min(4096.0 / f64::from(h))
                    .min((8_000_000.0 / (f64::from(w) * f64::from(h))).sqrt())
                    .min(1.0);
                self.view.pixels = (
                    ((f64::from(w) * limit) as u32).max(1),
                    ((f64::from(h) * limit) as u32).max(1),
                );
                self.protocol = None;
                self.changed();
            }
            if self.sent != Some(self.view) {
                if self.tx.send(self.view).is_err() {
                    self.busy = false;
                }
                self.sent = Some(self.view);
            }
            if let Some(protocol) = &self.protocol {
                f.render_widget(SlicedImage::new(protocol, (0, 0).into()), self.rect);
            }
            if let Some(error) = &self.error {
                f.render_widget(
                    Paragraph::new(format!("无法查看：{error}\nEsc 返回笔记"))
                        .wrap(Wrap { trim: false })
                        .style(Style::default().fg(palette.warn)),
                    self.rect,
                );
            } else if self.protocol.is_none() {
                f.render_widget(Paragraph::new("正在加载高清图片…"), self.rect);
            }
        }
        let heading = Rect::new(area.x, area.y, area.width.saturating_sub(12), 1);
        f.render_widget(
            Paragraph::new(format!(" 图片查看 · {}", self.title))
                .style(Style::default().fg(palette.accent)),
            heading,
        );
        f.render_widget(
            Paragraph::new(" Esc 关闭 ").style(Style::default().bg(palette.select)),
            self.close_button,
        );
        if area.height >= 2 {
            let status = format!(
                "相对适应 {:.0}%{} · +/- 缩放 · 0 适应 · 方向键/拖动平移",
                self.view.zoom * 100.0,
                if self.busy { " · 渲染中" } else { "" }
            );
            f.render_widget(
                Paragraph::new(status),
                Rect::new(area.x, area.bottom() - 2, area.width, 1),
            );
            f.render_widget(
                Paragraph::new(" O 系统打开 ")
                    .style(Style::default().fg(palette.accent).bg(palette.select)),
                self.open_button,
            );
            let rest = Rect::new(
                self.open_button.right(),
                self.open_button.y,
                area.width.saturating_sub(self.open_button.width),
                1,
            );
            f.render_widget(
                Paragraph::new(self.notice.as_deref().unwrap_or("滚轮平移 · Ctrl+滚轮缩放")),
                rest,
            );
        }
    }
}

impl App {
    pub(super) fn open_image(&mut self, url: Option<String>) -> Result<()> {
        let note = self.note.as_ref().context("请先打开笔记")?;
        let url = url
            .or_else(|| crate::layout::image_at(&note.doc.text, note.doc.cursor))
            .context("请将光标移到图片引用或 Mermaid 代码块，再按放大快捷键")?;
        let picker = self.images.viewer_picker(&url)?;
        self.viewer = Some(Viewer::new(
            note.path.clone(),
            url,
            picker,
            self.settings.clone(),
        ));
        self.dragging = false;
        self.resizing = None;
        Ok(())
    }
    fn open_viewer_external(&mut self) -> Result<()> {
        let viewer = self.viewer.as_ref().context("查看器未打开")?;
        let export = viewer.export.as_ref().context("图片尚未加载完成")?;
        if !export.path.is_file() {
            bail!("图片文件已不存在");
        }
        crate::links::open_external(
            &export.path.to_string_lossy(),
            self.options.opener.as_deref(),
        )?;
        if let Some(directory) = &export.directory
            && !self
                .viewer_exports
                .iter()
                .any(|d| Arc::ptr_eq(d, directory))
        {
            self.viewer_exports.push(directory.clone());
        }
        Ok(())
    }
    pub(super) fn viewer_key(&mut self, key: KeyEvent) {
        if key.modifiers.intersects(
            KeyModifiers::CONTROL
                | KeyModifiers::ALT
                | KeyModifiers::SUPER
                | KeyModifiers::HYPER
                | KeyModifiers::META,
        ) {
            return;
        }
        if key.code == KeyCode::Esc {
            self.viewer = None;
            return;
        }
        if matches!(key.code, KeyCode::Char('o' | 'O')) {
            let result = self.open_viewer_external();
            self.viewer.as_mut().unwrap().notice = Some(match result {
                Ok(()) => "已交给系统应用打开".into(),
                Err(e) => format!("{e:#}"),
            });
            return;
        }
        let viewer = self.viewer.as_mut().unwrap();
        let (w, h) = (
            f64::from(viewer.view.pixels.0),
            f64::from(viewer.view.pixels.1),
        );
        match key.code {
            KeyCode::Char('+' | '=') => viewer.zoom(1.25),
            KeyCode::Char('-') => viewer.zoom(0.8),
            KeyCode::Char('0') => {
                viewer.view.zoom = 1.0;
                viewer.view.center = (0.5, 0.5);
                viewer.changed();
            }
            KeyCode::Left => viewer.pan(-w / 10.0, 0.0),
            KeyCode::Right => viewer.pan(w / 10.0, 0.0),
            KeyCode::Up => viewer.pan(0.0, -h / 10.0),
            KeyCode::Down => viewer.pan(0.0, h / 10.0),
            _ => {}
        }
    }
    pub(super) fn viewer_mouse(&mut self, event: MouseEvent) {
        let position = (event.column, event.row).into();
        let viewer = self.viewer.as_mut().unwrap();
        match event.kind {
            MouseEventKind::Down(MouseButton::Left) if viewer.close_button.contains(position) => {
                self.viewer = None
            }
            MouseEventKind::Down(MouseButton::Left) if viewer.open_button.contains(position) => {
                self.viewer_key(KeyEvent::new(KeyCode::Char('o'), KeyModifiers::NONE))
            }
            MouseEventKind::Down(MouseButton::Left) if viewer.rect.contains(position) => {
                viewer.drag = Some((event.column, event.row))
            }
            MouseEventKind::Drag(MouseButton::Left) => {
                if let Some((x, y)) = viewer.drag {
                    let dx = (f64::from(x) - f64::from(event.column))
                        * f64::from(viewer.view.pixels.0)
                        / f64::from(viewer.rect.width.max(1));
                    let dy = (f64::from(y) - f64::from(event.row))
                        * f64::from(viewer.view.pixels.1)
                        / f64::from(viewer.rect.height.max(1));
                    viewer.pan(dx, dy);
                    viewer.drag = Some((event.column, event.row));
                }
            }
            MouseEventKind::Up(MouseButton::Left) => viewer.drag = None,
            MouseEventKind::ScrollUp | MouseEventKind::ScrollDown
                if viewer.rect.contains(position) =>
            {
                let direction = if event.kind == MouseEventKind::ScrollUp {
                    -1.0
                } else {
                    1.0
                };
                if event.modifiers.contains(KeyModifiers::CONTROL) {
                    viewer.zoom(if direction < 0.0 { 1.25 } else { 0.8 });
                } else {
                    viewer.pan(0.0, direction * f64::from(viewer.view.pixels.1) / 8.0);
                }
            }
            MouseEventKind::ScrollLeft => viewer.pan(-f64::from(viewer.view.pixels.0) / 8.0, 0.0),
            MouseEventKind::ScrollRight => viewer.pan(f64::from(viewer.view.pixels.0) / 8.0, 0.0),
            _ => {}
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn viewport_crops_original_detail_and_clamps_edges() {
        let image = DynamicImage::ImageRgba8(image::RgbaImage::from_fn(200, 100, |x, _| {
            if x < 100 {
                image::Rgba([255, 0, 0, 255])
            } else {
                image::Rgba([0, 0, 255, 255])
            }
        }));
        let source = Source::Raster(image);
        let mut view = View {
            cells: Size::new(10, 5),
            pixels: (100, 50),
            zoom: 4.0,
            center: (0.0, 0.5),
            generation: 0,
        };
        let cancel = AtomicBool::new(false);
        let left = source.viewport(&view, &cancel).unwrap().to_rgba8();
        assert_eq!(left.dimensions(), (100, 50));
        assert_eq!(*left.get_pixel(50, 25), image::Rgba([255, 0, 0, 255]));
        view.center.0 = 1.0;
        let right = source.viewport(&view, &cancel).unwrap().to_rgba8();
        assert_eq!(*right.get_pixel(50, 25), image::Rgba([0, 0, 255, 255]));
        view.zoom = 0.25;
        let small = source.viewport(&view, &cancel).unwrap().to_rgba8();
        assert_eq!(small.get_pixel(0, 0)[3], 0);
    }
}
