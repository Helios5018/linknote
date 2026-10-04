//! Offline Mermaid rendering. Keys include the complete source so edits cannot
//! reuse a stale diagram, and never overlap with a real image destination.
use anyhow::{Context, Result, bail};
use image::DynamicImage;
use pulldown_cmark::{CodeBlockKind, Event, Tag, TagEnd};
use serde::{Deserialize, Serialize};
use std::{
    fs,
    io::{Read, Seek},
    ops::Range,
    path::PathBuf,
    process::{Command, Stdio},
    sync::atomic::{AtomicBool, Ordering},
    time::{Duration, Instant},
};

const PREFIX: &str = "\0mermaid:";
pub const MAX_ROWS: usize = 80;

pub fn source(key: &str) -> Option<&str> {
    key.strip_prefix(PREFIX)
}

pub(crate) fn is_mermaid(tag: &Tag<'_>) -> bool {
    matches!(tag, Tag::CodeBlock(CodeBlockKind::Fenced(info))
        if info.split_whitespace().next().is_some_and(|s| s.eq_ignore_ascii_case("mermaid")))
}

pub(crate) fn references(
    events: &[(Event<'_>, Range<usize>)],
) -> Vec<(String, Range<usize>, bool)> {
    let mut out = vec![];
    let mut current: Option<(String, Range<usize>)> = None;
    for (event, range) in events {
        match event {
            Event::Start(tag) if is_mermaid(tag) => {
                current = Some((String::from(PREFIX), range.clone()));
            }
            Event::Text(text) => {
                if let Some((body, _)) = &mut current {
                    body.push_str(text);
                }
            }
            Event::End(TagEnd::CodeBlock) => {
                if let Some((key, range)) = current.take() {
                    out.push((key, range, false));
                }
            }
            _ => {}
        }
    }
    out
}

/// Render through the bundled official Mermaid engine. The worker blocks all
/// page requests, exits after each render, and is killed as a process group if it
/// exceeds its deadline. No shell interprets the diagram source.
pub fn render(source: &str) -> Result<DynamicImage> {
    render_with_settings(source, &crate::settings::Settings::default())
}

pub fn render_with_settings(
    source: &str,
    settings: &crate::settings::Settings,
) -> Result<DynamicImage> {
    decode_png(run(source, settings, &serde_json::json!({}), None)?)
}

#[derive(Clone, Serialize, Deserialize)]
pub struct Vector {
    pub svg: String,
    pub width: f64,
    pub height: f64,
    pub background: String,
}

pub fn render_vector(
    source: &str,
    settings: &crate::settings::Settings,
    cancel: &AtomicBool,
) -> Result<Vector> {
    let bytes = run(
        source,
        settings,
        &serde_json::json!({"mode": "vector"}),
        Some(cancel),
    )?;
    Ok(serde_json::from_slice(&bytes)?)
}

pub fn render_viewport(
    vector: &Vector,
    viewport: serde_json::Value,
    cancel: &AtomicBool,
) -> Result<DynamicImage> {
    decode_png(run(
        &serde_json::to_string(vector)?,
        &crate::settings::Settings::default(),
        &serde_json::json!({"mode":"viewport", "viewport":viewport}),
        Some(cancel),
    )?)
}

fn run(
    source: &str,
    settings: &crate::settings::Settings,
    options: &serde_json::Value,
    cancel: Option<&AtomicBool>,
) -> Result<Vec<u8>> {
    if cancel.is_some_and(|c| c.load(Ordering::Relaxed)) {
        bail!("已关闭图片查看器");
    }
    if options["mode"] != "viewport" && (source.len() > 32 * 1024 || source.lines().count() > 512) {
        bail!("图表超过 32 KiB 或 512 行，请拆分图表");
    }
    let renderer = std::env::var_os("LINKNOTE_MERMAID_DIR")
        .map(PathBuf::from)
        .unwrap_or_else(|| PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("scripts/mermaid"));
    if !renderer.join("bundle.js").is_file() || !renderer.join("node_modules").is_dir() {
        bail!("请先在 LinkNote 项目目录执行 npm ci --prefix scripts/mermaid");
    }
    let temp = tempfile::tempdir()?;
    let input = temp.path().join("input.json");
    let output = temp.path().join("diagram.png");
    let stderr = tempfile::tempfile()?;
    fs::write(&input, source)?;
    let mut command = Command::new("node");
    command
        .arg(renderer.join("render.mjs"))
        .arg(&input)
        .arg(&output)
        .arg(serde_json::to_string(settings)?)
        .arg(serde_json::to_string(options)?)
        .stdin(Stdio::null())
        .stdout(Stdio::null())
        .stderr(stderr.try_clone()?);
    #[cfg(unix)]
    {
        use std::os::unix::process::CommandExt;
        command.process_group(0);
    }
    let mut child = command
        .spawn()
        .context("无法启动 Mermaid：请安装 Node.js 22.12 或更高版本")?;
    let started = Instant::now();
    let status = loop {
        match child.try_wait() {
            Ok(Some(status)) => break status,
            Ok(None)
                if started.elapsed() < Duration::from_secs(25)
                    && !cancel.is_some_and(|c| c.load(Ordering::Relaxed)) =>
            {
                std::thread::sleep(Duration::from_millis(25));
            }
            result => {
                #[cfg(unix)]
                // SAFETY: this child was started in its own process group. Kill
                // only that render's Node/Chromium children, never the host app.
                unsafe {
                    libc::kill(-(child.id() as i32), libc::SIGKILL);
                }
                let _ = child.kill();
                let _ = child.wait();
                if let Err(error) = result {
                    return Err(error.into());
                }
                bail!("图表渲染超时，请简化图表后重试");
            }
        }
    };
    if !status.success() {
        let mut error = String::new();
        let mut stderr = stderr;
        stderr.rewind()?;
        stderr.take(4096).read_to_string(&mut error)?;
        let error: String = error
            .chars()
            .filter(|c| !c.is_control() || *c == '\n')
            .collect();
        bail!(
            "{}",
            if error.trim().is_empty() {
                "图表渲染失败"
            } else {
                error.trim()
            }
        );
    }
    fs::read(output).context("无法读取 Mermaid 输出")
}

fn decode_png(bytes: Vec<u8>) -> Result<DynamicImage> {
    let mut reader = image::ImageReader::new(std::io::Cursor::new(bytes)).with_guessed_format()?;
    let mut limits = image::Limits::default();
    limits.max_image_width = Some(4096);
    limits.max_image_height = Some(4096);
    limits.max_alloc = Some(32 * 1024 * 1024);
    reader.limits(limits);
    reader.decode().context("无法读取 Mermaid 图像")
}
