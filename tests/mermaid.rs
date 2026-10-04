use linknote::{
    document::Document,
    images::Images,
    layout::{Layout, image_sources},
    mermaid,
};
use ratatui_image::picker::Picker;
use std::{
    collections::HashMap,
    fs,
    path::Path,
    time::{Duration, Instant},
};

#[test]
fn all_23_demo_types_render_nonempty_local_images() {
    let dir = Path::new(env!("CARGO_MANIFEST_DIR")).join("docs/demo/Mermaid");
    let mut paths: Vec<_> = fs::read_dir(dir)
        .unwrap()
        .map(|e| e.unwrap().path())
        .collect();
    paths.sort();
    let mut count = 0;
    let mut failures = vec![];
    for path in paths {
        let name = path.file_name().unwrap().to_str().unwrap();
        if name.starts_with("00-") || name.starts_with("24-") {
            continue;
        }
        let text = fs::read_to_string(&path).unwrap();
        let keys = image_sources(&text);
        assert_eq!(keys.len(), 1, "{name}");
        let body = mermaid::source(&keys[0]).unwrap();
        match mermaid::render(body) {
            Ok(image) => {
                assert!(image.width() > 20 && image.height() > 20, "{name}");
                assert!(image.width() <= 1601 && image.height() <= 2401);
                let rgba = image.to_rgba8();
                let first = rgba.get_pixel(0, 0);
                let drawn = rgba.pixels().filter(|p| *p != first).count();
                assert!(drawn > 100, "empty canvas: {name}");
                if let Ok(dir) = std::env::var("LINKNOTE_MERMAID_ARTIFACT_DIR") {
                    fs::create_dir_all(&dir).unwrap();
                    image
                        .save(Path::new(&dir).join(format!("{name}.png")))
                        .unwrap();
                }
            }
            Err(e) => failures.push(format!("{name}: {e:#}")),
        }
        count += 1;
    }
    assert_eq!(count, 23);
    assert!(failures.is_empty(), "{}", failures.join("\n"));
}

#[test]
fn fences_preserve_source_and_only_inactive_diagrams_get_canvas_rows() {
    let text = "# 标题\n\n```mermaid\nflowchart LR\n A[中文] --> B[图表]\n```\n\n结尾\n";
    let mut doc = Document::new(text.into());
    let keys = image_sources(text);
    assert_eq!(
        mermaid::source(&keys[0]),
        Some("flowchart LR\n A[中文] --> B[图表]\n")
    );
    let heights = HashMap::from([(keys[0].clone(), 40)]);
    let layout = Layout::with_images(&doc, 60, &heights);
    assert_eq!(layout.images.len(), 1);
    let slot = &layout.images[0];
    assert_eq!(
        slot.height, 40,
        "tall diagrams scroll instead of getting cut to 24 rows"
    );
    assert!(!layout.plain().contains("A[中文]"));
    for row in slot.row..slot.row + slot.height {
        assert_eq!(layout.hit(row, 10), text.find("```mermaid").unwrap());
    }
    doc.set_cursor(text.find("中文").unwrap(), false);
    let editing = Layout::update(Some(layout), &doc, 60, &heights);
    assert!(editing.images.is_empty());
    assert!(editing.plain().contains("```mermaid"));
    assert!(editing.plain().contains("A[中文]"));
    doc.set_cursor(text.len(), false);
    let preview = Layout::update(Some(editing), &doc, 60, &heights);
    assert_eq!(preview.images.len(), 1);
    assert_eq!(doc.text, text);
}

#[test]
fn nested_duplicate_tilde_and_unclosed_fences_keep_correct_mapping() {
    let text = "# Heading\n\n> ~~~MERMAID\n> flowchart LR\n> A --> B\n> ~~~\n\n- item\n\n  ```mermaid\n  flowchart LR\n  A --> B\n  ```\n\n```text\nflowchart LR\nA --> B\n```\n\n```mermaid\nflowchart LR\nA --> B\n";
    let mut doc = Document::new(text.into());
    let keys = image_sources(text);
    assert_eq!(keys.len(), 3);
    assert!(keys.iter().all(|k| k == &keys[0]));
    let heights = HashMap::from([(keys[0].clone(), 4)]);
    let layout = Layout::with_images(&doc, 70, &heights);
    assert_eq!(layout.images.len(), 3);
    assert!(
        layout.plain().contains("A --> B"),
        "ordinary code is preserved"
    );
    let source = layout.images[1].source.start;
    doc.set_cursor(source, false);
    let layout = Layout::update(Some(layout), &doc, 70, &heights);
    assert_eq!(layout.images.len(), 2);
    assert!(layout.plain().contains("  ```mermaid"));
    assert_eq!(doc.text, text);
}

#[test]
fn malformed_unsupported_empty_and_oversized_sources_return_errors() {
    for source in [
        "",
        "unknownDiagram\n A --> B",
        "flowchart TD\n A[unclosed",
        &"x".repeat(32769),
    ] {
        assert!(mermaid::render(source).is_err(), "{source}");
    }
}

#[test]
fn preferences_change_canvas_theme_font_size_and_diagram_overrides_win() {
    use linknote::settings::{Canvas, MermaidTheme, Settings, Theme};
    let source = "flowchart LR\n A[Start] --> B[End]\n";
    let mut settings = Settings::default();
    let dark = mermaid::render_with_settings(source, &settings)
        .unwrap()
        .to_rgba8();
    assert_eq!(dark.get_pixel(0, 0).0, [22, 27, 34, 255]);
    settings.theme = Theme::Light;
    let light = mermaid::render_with_settings(source, &settings)
        .unwrap()
        .to_rgba8();
    assert_eq!(light.get_pixel(0, 0).0, [255, 255, 255, 255]);
    assert_ne!(dark, light);
    settings.mermaid_canvas = Canvas::Transparent;
    let transparent = mermaid::render_with_settings(source, &settings)
        .unwrap()
        .to_rgba8();
    assert_eq!(transparent.get_pixel(0, 0).0[3], 0);
    settings.mermaid_canvas = Canvas::Auto;
    settings.mermaid_theme = MermaidTheme::Dark;
    let override_source = format!("---\nconfig:\n  theme: forest\n---\n{source}");
    let overridden = mermaid::render_with_settings(&override_source, &settings)
        .unwrap()
        .to_rgba8();
    assert_eq!(overridden.get_pixel(0, 0).0, [255, 255, 255, 255]);
    settings.mermaid_font_size = 28;
    let large = mermaid::render_with_settings(source, &settings).unwrap();
    assert!(
        large.height() > dark.height(),
        "the configured font size must affect the actual diagram"
    );
}

#[test]
fn source_edits_resize_and_network_permission_changes_do_not_leave_stale_jobs() {
    let note = Path::new("note.md");
    let mut images = Images::new(Picker::halfblocks());
    let first = "# Start\n\n```mermaid\nflowchart LR\n A --> B\n```\n";
    let changed = first.replace("A --> B", "A --> C --> D");
    let first_key = image_sources(first).remove(0);
    let changed_key = image_sources(&changed).remove(0);
    images.sync(note, 0, first, 60);
    images.set_remote(true);
    images.sync(note, 1, &changed, 60);
    images.sync(note, 1, &changed, 30);
    let deadline = Instant::now() + Duration::from_secs(10);
    while images.heights().get(&changed_key).copied().unwrap_or(0) <= 3 && Instant::now() < deadline
    {
        images.poll();
        std::thread::sleep(Duration::from_millis(10));
    }
    assert!(!images.has_blocked());
    assert!(!images.heights().contains_key(&first_key));
    assert!(images.heights()[&changed_key] > 3);
}
