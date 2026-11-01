//! The datasheet renderer: `hydra-mcp render-pdf` run as a child, as read_datasheet runs it.

use std::path::Path;
use std::time::{Duration, Instant};

use base64::Engine;
use hydra_mcp::{DatasheetParams, MAX_PX, RENDER_TIMEOUT, read_pdf, run_renderer};
use rmcp::model::CallToolResult;
use tokio::process::Command;

const BIN: &str = env!("CARGO_BIN_EXE_hydra-mcp");
const PDF: &str = concat!(
    env!("CARGO_MANIFEST_DIR"),
    "/../../fixtures/datasheets/demo-10-datasheet.pdf"
);
/// Text and base64 of one default call on the demo datasheet, in bytes: about 73 KB with
/// 4-bit grayscale pages (8-bit would be about 125 KB).
const DEMO_BUDGET: usize = 76_000;

fn params(first_page: Option<u32>) -> DatasheetParams {
    DatasheetParams {
        key: "vt-demo-10".into(),
        first_page,
        last_page: None,
        max_pages: None,
    }
}

async fn read(pdf: Vec<u8>) -> Result<CallToolResult, String> {
    read_pdf(Path::new(BIN), "vt-demo-10@1", pdf, &params(None)).await
}

fn text(r: &CallToolResult) -> &str {
    &r.content[0].as_text().expect("text first").text
}

/// Width, height and 8-bit gray pixels of a grayscale PNG.
fn decode(b64: &str) -> (u32, u32, Vec<u8>) {
    let png = base64::engine::general_purpose::STANDARD
        .decode(b64)
        .unwrap();
    let mut decoder = png::Decoder::new(std::io::Cursor::new(png));
    decoder.set_transformations(png::Transformations::EXPAND);
    let mut reader = decoder.read_info().unwrap();
    assert_eq!(reader.info().color_type, png::ColorType::Grayscale);
    let mut buf = vec![0; reader.output_buffer_size().unwrap()];
    let frame = reader.next_frame(&mut buf).unwrap();
    assert_eq!(frame.bit_depth, png::BitDepth::Eight, "expanded");
    buf.truncate(frame.buffer_size());
    (frame.width, frame.height, buf)
}

#[tokio::test]
async fn the_demo_datasheet_renders_with_its_text() {
    let mut cmd = Command::new(BIN);
    cmd.args(["render-pdf", "1", "1", "2", &MAX_PX.to_string()]);
    let out = run_renderer(cmd, std::fs::read(PDF).unwrap(), RENDER_TIMEOUT)
        .await
        .unwrap();
    assert_eq!(out.pages, 2);
    assert!(
        out.image_errors.is_empty() && out.text_error.is_none(),
        "{out:?}"
    );
    let pages = out.text.unwrap();
    for text in [
        "4/3 directional valve",
        "ISO 4401-05",
        "315 bar",
        "120 l/min",
    ] {
        assert!(pages[0].contains(text), "{text} not in {:?}", pages[0]);
    }
    for text in ["DEMO-10-4-3-C-24DC", "DEMO-10-4-3-C-230AC", "EN 175301-803"] {
        assert!(pages[1].contains(text), "{text} not in {:?}", pages[1]);
    }
    assert_eq!(
        out.images.iter().map(|i| i.page).collect::<Vec<_>>(),
        [1, 2]
    );
    for image in &out.images {
        let (w, h, gray) = decode(&image.png_base64);
        assert_eq!(w.max(h), MAX_PX, "{w}x{h}");
        // A white A4 page with black text.
        assert!(h > w && gray.contains(&0) && gray.contains(&255));
    }
}

#[tokio::test]
async fn a_default_call_fits_the_budget() {
    let r = read(std::fs::read(PDF).unwrap()).await.unwrap();
    assert_eq!(r.content.len(), 3, "text and two pages");
    let t = text(&r);
    assert!(t.starts_with("Datasheet of vt-demo-10@1, 2 pages."), "{t}");
    assert!(
        t.contains("315 bar") && t.contains("DEMO-10-4-3-C-230AC"),
        "{t}"
    );
    let size: usize = r
        .content
        .iter()
        .map(|c| match (c.as_text(), c.as_image()) {
            (Some(t), _) => t.text.len(),
            (_, Some(i)) => i.data.len(),
            _ => panic!("{c:?}"),
        })
        .sum();
    assert!(size < DEMO_BUDGET, "{size} bytes");

    // first_page and max_pages pick the pages.
    let p = DatasheetParams {
        max_pages: Some(1),
        ..params(Some(2))
    };
    let r = read_pdf(
        Path::new(BIN),
        "vt-demo-10@1",
        std::fs::read(PDF).unwrap(),
        &p,
    )
    .await
    .unwrap();
    assert_eq!(r.content.len(), 2, "{r:?}");
    assert!(!text(&r).contains("--- page 1 ---") && text(&r).contains("--- page 2 ---"));
}

#[tokio::test]
async fn garbage_is_an_error_and_the_next_call_works() {
    let e = read(b"not a PDF at all".to_vec()).await.unwrap_err();
    assert!(e.starts_with("could not read the datasheet:"), "{e}");
    assert!(read(std::fs::read(PDF).unwrap()).await.is_ok());
}

/// Three pages that only draw an image: no text operators at all.
fn image_only_pdf() -> Vec<u8> {
    use lopdf::{Document, Object, Stream, dictionary};
    let mut doc = Document::with_version("1.5");
    let pages = doc.new_object_id();
    let image = doc.add_object(Stream::new(
        dictionary! {
            "Type" => "XObject", "Subtype" => "Image", "Width" => 2, "Height" => 2,
            "ColorSpace" => "DeviceGray", "BitsPerComponent" => 8,
        },
        vec![0, 255, 255, 0],
    ));
    let content = doc.add_object(Stream::new(
        dictionary! {},
        b"q 400 0 0 400 100 200 cm /Im0 Do Q".to_vec(),
    ));
    let kids: Vec<Object> = (0..3)
        .map(|_| {
            doc.add_object(dictionary! {
                "Type" => "Page", "Parent" => pages, "Contents" => content,
                "Resources" => dictionary! { "XObject" => dictionary! { "Im0" => image } },
            })
            .into()
        })
        .collect();
    doc.objects.insert(
        pages,
        Object::Dictionary(dictionary! {
            "Type" => "Pages", "Kids" => kids, "Count" => 3,
            "MediaBox" => vec![0.into(), 0.into(), 595.into(), 842.into()],
        }),
    );
    let catalog = doc.add_object(dictionary! { "Type" => "Catalog", "Pages" => pages });
    doc.trailer.set("Root", catalog);
    let mut out = Vec::new();
    doc.save_to(&mut out).unwrap();
    out
}

#[tokio::test]
async fn a_pdf_without_text_still_gives_its_pages() {
    let r = read(image_only_pdf()).await.unwrap();
    let t = text(&r);
    assert!(
        t.starts_with("Datasheet of vt-demo-10@1, 3 pages.") && t.contains("[Text unavailable:"),
        "{t}"
    );
    // Two pages by default, and a note on the third.
    let [_, a, b] = &r.content[..] else {
        panic!("{r:?}")
    };
    assert!(
        t.contains("from 1 to 2") && t.contains("first_page=3"),
        "{t}"
    );
    for image in [a, b] {
        let (_, _, gray) = decode(&image.as_image().unwrap().data);
        assert!(
            gray.contains(&0) && gray.contains(&255),
            "the image is drawn"
        );
    }

    let p = DatasheetParams {
        max_pages: Some(4),
        ..params(None)
    };
    let r = read_pdf(Path::new(BIN), "vt-demo-10@1", image_only_pdf(), &p)
        .await
        .unwrap();
    assert_eq!(r.content.len(), 4, "text and all three pages");
    assert!(!text(&r).contains("first_page="), "{}", text(&r));
}

#[cfg(unix)]
#[tokio::test]
async fn a_stuck_renderer_is_killed() {
    // An odd duration, to find this very child below.
    let mut cmd = Command::new("sleep");
    cmd.arg("5.0371");
    let started = Instant::now();
    let e = run_renderer(cmd, Vec::new(), Duration::from_millis(200))
        .await
        .unwrap_err();
    assert!(e.contains("took too long"), "{e}");
    assert!(started.elapsed() < Duration::from_secs(2));
    let pgrep = std::process::Command::new("pgrep")
        .args(["-f", "sleep 5.0371"])
        .output()
        .unwrap();
    assert!(!pgrep.status.success(), "still running: {pgrep:?}");
}

#[cfg(unix)]
#[tokio::test]
async fn a_flood_of_output_is_cut_off_at_once() {
    let started = Instant::now();
    let e = run_renderer(Command::new("yes"), Vec::new(), Duration::from_secs(30))
        .await
        .unwrap_err();
    assert!(e.contains("too large"), "{e}");
    assert!(
        started.elapsed() < Duration::from_secs(10),
        "it waited for the timeout"
    );
}

#[test]
fn another_protocol_version_is_refused() {
    let out = std::process::Command::new(BIN)
        .args(["render-pdf", "2", "1", "2", "1568"])
        .output()
        .unwrap();
    assert!(!out.status.success());
    assert!(out.stdout.is_empty());
    let err = String::from_utf8_lossy(&out.stderr);
    assert!(err.contains("protocol version 2 is not supported"), "{err}");
}
