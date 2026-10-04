use image::{DynamicImage, ImageFormat, Rgba, RgbaImage};
use linknote::{
    document::Document,
    images::{Images, load_image},
    layout::Layout,
};
use ratatui::{
    Terminal,
    backend::TestBackend,
    layout::{Rect, Size},
};
use ratatui_image::{
    picker::{Picker, ProtocolType},
    sliced::{SlicedImage, SlicedProtocol},
};
use std::{
    collections::HashMap,
    fs,
    io::{Cursor, Read, Write},
    net::TcpListener,
    time::{Duration, Instant},
};

fn png() -> Vec<u8> {
    let image = DynamicImage::ImageRgba8(RgbaImage::from_fn(80, 80, |x, y| {
        Rgba([(x * 3) as u8, (y * 3) as u8, 100, 255])
    }));
    let mut out = Cursor::new(vec![]);
    image.write_to(&mut out, ImageFormat::Png).unwrap();
    out.into_inner()
}

#[test]
fn local_unicode_encoded_paths_decode_and_bad_images_report_errors() {
    let dir = tempfile::tempdir().unwrap();
    let note = dir.path().join("note.md");
    fs::write(dir.path().join("中文 图.png"), png()).unwrap();
    let img = load_image(&note, "中文%20图.png").unwrap();
    assert_eq!((img.width(), img.height()), (80, 80));
    fs::write(dir.path().join("bad.png"), b"not an image").unwrap();
    assert!(load_image(&note, "bad.png").is_err());
    assert!(load_image(&note, "absent.png").is_err());
    assert!(load_image(&note, "file://secret").is_err());
}

#[test]
fn http_image_download_and_decode() {
    let listener = TcpListener::bind("127.0.0.1:0").unwrap();
    let url = format!("http://{}/image.png", listener.local_addr().unwrap());
    let payload = png();
    let server = std::thread::spawn(move || {
        let (mut socket, _) = listener.accept().unwrap();
        socket
            .set_read_timeout(Some(Duration::from_secs(3)))
            .unwrap();
        let mut buffer = [0; 4096];
        assert!(socket.read(&mut buffer).unwrap() > 0);
        write!(socket,"HTTP/1.1 200 OK\r\nContent-Type: image/png\r\nContent-Length: {}\r\nConnection: close\r\n\r\n",payload.len()).unwrap();
        socket.write_all(&payload).unwrap();
    });
    let image = load_image(std::path::Path::new("note.md"), &url).unwrap();
    assert_eq!(image.width(), 80);
    server.join().unwrap();
}

#[test]
fn images_reserve_rows_scroll_clip_and_reveal_editable_source() {
    let source = "before\n\n![图](a.png)\n\nafter\n";
    let mut doc = Document::new(source.into());
    let heights = HashMap::from([("a.png".into(), 4)]);
    let layout = Layout::with_images(&doc, 60, &heights);
    assert_eq!(layout.images.len(), 1);
    let slot = &layout.images[0];
    for row in slot.row..slot.row + slot.height {
        assert_eq!(layout.hit(row, 10), source.find("![").unwrap());
    }
    doc.set_cursor(layout.hit(slot.row, 0), false);
    let editing = Layout::with_images(&doc, 60, &heights);
    assert!(editing.images.is_empty());
    assert!(editing.plain().contains("![图](a.png)"));
    assert_eq!(doc.text, source);

    let doc = Document::new("![一](a.png) ![二](b.png)\n\nend".into());
    let mut doc = doc;
    doc.set_cursor(doc.text.len(), false);
    let heights = HashMap::from([("a.png".into(), 3), ("b.png".into(), 4)]);
    let multiple = Layout::with_images(&doc, 60, &heights);
    assert_eq!(multiple.images.len(), 2);
    assert_eq!(multiple.images[0].url, "a.png");
    assert!(multiple.images[0].row + multiple.images[0].height <= multiple.images[1].row);

    let protocol = SlicedProtocol::new(
        &Picker::halfblocks(),
        image::load_from_memory(&png()).unwrap(),
        Some(Size::new(20, 8)),
    )
    .unwrap();
    let mut terminal = Terminal::new(TestBackend::new(30, 12)).unwrap();
    terminal
        .draw(|f| {
            f.render_widget(
                SlicedImage::new(&protocol, (0, -1).into()),
                Rect::new(2, 2, 20, 8),
            )
        })
        .unwrap();
    let b = terminal.backend().buffer();
    assert_ne!(b[(2, 2)].bg, b[(0, 0)].bg);
    assert_eq!(b[(1, 2)].symbol(), " ");
    assert_eq!(b[(2, 1)].symbol(), " ");
}

#[test]
fn native_image_protocols_encode_and_async_cache_updates_layout_height() {
    for kind in [
        ProtocolType::Kitty,
        ProtocolType::Sixel,
        ProtocolType::Iterm2,
    ] {
        let mut picker = Picker::halfblocks();
        picker.set_protocol_type(kind);
        let p = SlicedProtocol::new(
            &picker,
            image::load_from_memory(&png()).unwrap(),
            Some(Size::new(20, 8)),
        )
        .unwrap();
        assert!(p.size().width > 0 && p.size().height > 0);
    }
    let dir = tempfile::tempdir().unwrap();
    fs::write(dir.path().join("a.png"), png()).unwrap();
    let mut cache = Images::new(Picker::halfblocks());
    cache.sync(&dir.path().join("note.md"), 0, "![图](a.png)", 60);
    let deadline = Instant::now() + Duration::from_secs(4);
    while cache.heights().get("a.png").copied().unwrap_or(0) <= 1 && Instant::now() < deadline {
        cache.poll();
        std::thread::sleep(Duration::from_millis(10));
    }
    assert!(cache.heights()["a.png"] > 1);
    cache.sync(&dir.path().join("note.md"), 0, "![图](a.png)", 20);
    while cache.heights()["a.png"] <= 1 && Instant::now() < deadline {
        cache.poll();
        std::thread::sleep(Duration::from_millis(10));
    }
    assert!(cache.heights()["a.png"] > 1);
}

#[test]
fn network_images_wait_for_permission_before_any_request() {
    let listener = TcpListener::bind("127.0.0.1:0").unwrap();
    listener.set_nonblocking(true).unwrap();
    let url = format!("http://{}/image.png", listener.local_addr().unwrap());
    let mut cache = Images::new(Picker::halfblocks());
    let note = std::path::Path::new("note.md");
    cache.sync(note, 0, &format!("![远程]({url})"), 60);
    assert!(cache.has_blocked());
    let deadline = Instant::now() + Duration::from_millis(300);
    while Instant::now() < deadline {
        cache.poll();
        assert!(listener.accept().is_err(), "no request before permission");
        std::thread::sleep(Duration::from_millis(20));
    }
    assert_eq!(cache.heights()[&url], 1, "placeholder row");

    let payload = png();
    listener.set_nonblocking(false).unwrap();
    let server = std::thread::spawn(move || {
        let (mut socket, _) = listener.accept().unwrap();
        let mut buffer = [0; 4096];
        assert!(socket.read(&mut buffer).unwrap() > 0);
        write!(
            socket,
            "HTTP/1.1 200 OK\r\nContent-Length: {}\r\nConnection: close\r\n\r\n",
            payload.len()
        )
        .unwrap();
        socket.write_all(&payload).unwrap();
    });
    cache.set_remote(true);
    let deadline = Instant::now() + Duration::from_secs(4);
    while cache.heights()[&url] <= 1 && Instant::now() < deadline {
        cache.poll();
        std::thread::sleep(Duration::from_millis(10));
    }
    assert!(cache.heights()[&url] > 1);
    assert!(!cache.has_blocked());
    server.join().unwrap();
}

#[test]
fn viewer_decodes_full_resolution_and_permission_gate_applies_to_native_picker() {
    let dir = tempfile::tempdir().unwrap();
    let note = dir.path().join("note.md");
    DynamicImage::new_rgba8(1800, 900)
        .save(dir.path().join("large.png"))
        .unwrap();
    assert_eq!(load_image(&note, "large.png").unwrap().width(), 1200);
    assert_eq!(
        linknote::images::load_full_image(&note, "large.png")
            .unwrap()
            .width(),
        1800
    );
    let mut images = Images::new(Picker::halfblocks());
    assert!(
        images
            .viewer_picker("https://example.invalid/image.png")
            .is_err()
    );
    images.set_remote(true);
    assert!(
        images
            .viewer_picker("https://example.invalid/image.png")
            .is_ok()
    );
    images.set_remote(false);
    assert!(
        images
            .viewer_picker("https://example.invalid/image.png")
            .is_err()
    );
    let settings = linknote::settings::Settings {
        image_display: linknote::settings::ImageDisplay::Off,
        ..Default::default()
    };
    images.configure(&settings);
    assert!(images.viewer_picker("large.png").is_err());
}

#[test]
fn viewer_targets_inline_reference_images_and_nested_mermaid_but_not_links_or_code() {
    let text = "![one](one.png) ![two][ref]\n\n[ref]: two.png\n\n> ~~~mermaid\n> flowchart LR\n> A --> B\n> ~~~\n\n[link](one.png)\n\n`![code](code.png)`\n";
    let at = |s| linknote::layout::image_at(text, text.find(s).unwrap());
    assert_eq!(at("one]"), Some("one.png".into()));
    assert_eq!(at("two]"), Some("two.png".into()));
    assert!(linknote::mermaid::source(&at("A --> B").unwrap()).is_some());
    assert_eq!(at("link]"), None);
    assert_eq!(at("code]"), None);
}
