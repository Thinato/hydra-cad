//! `hydra-mcp`: the MCP server over stdio. stdout carries the protocol; logs go to stderr.
//!
//! `hydra-mcp render-pdf ...` is the datasheet renderer the server runs as its own child.

use std::io::{Read, Write};
use std::panic::{AssertUnwindSafe, catch_unwind};
use std::process::ExitCode;

use base64::Engine;
use hayro::hayro_interpret::InterpreterSettings;
use hayro::hayro_syntax::Pdf;
use hayro::hayro_syntax::page::Page;
use hayro::vello_cpu::color::palette::css::WHITE;
use hayro::{PixmapSettings, RenderCache, RenderSettings};
use hydra_mcp::{HydraMcp, MAX_PDF_BYTES, PageImage, RENDER_PROTOCOL, Rendered};
use rmcp::ServiceExt;

fn main() -> ExitCode {
    let args: Vec<String> = std::env::args().skip(1).collect();
    if args.first().is_some_and(|a| a == "render-pdf") {
        return render_pdf(&args[1..]);
    }
    tracing_subscriber::fmt()
        .with_writer(std::io::stderr)
        .with_ansi(false)
        .init();
    serve()
}

#[tokio::main]
async fn serve() -> ExitCode {
    match run().await {
        Ok(()) => ExitCode::SUCCESS,
        Err(e) => {
            eprintln!("hydra-mcp: {e}");
            ExitCode::FAILURE
        }
    }
}

async fn run() -> Result<(), String> {
    let url = std::env::var("HYDRA_DATABASE_URL").map_err(|_| {
        "HYDRA_DATABASE_URL is not set; set it to the Hydra database, \
         e.g. postgres://user:password@server:5433/hydra"
            .to_string()
    })?;
    // The datasheet renderer is this same program.
    let renderer = std::env::current_exe()
        .map_err(|e| format!("cannot find the hydra-mcp program to read datasheets with: {e}"))?;
    let cannot = |e: hydra_db::Error| format!("cannot connect to the Hydra database: {e}");
    let options = url
        .parse::<hydra_db::PgConnectOptions>()
        .map_err(|e| cannot(e.into()))?;
    // Shows in the Account tab's session list.
    let pool = hydra_db::connect_with(options.application_name("hydra-mcp"))
        .await
        .map_err(cannot)?;
    hydra_db::check_schema(&pool)
        .await
        .map_err(|e| format!("the Hydra database can't be used: {e}"))?;
    let service = HydraMcp::new(pool, renderer)
        .serve(rmcp::transport::stdio())
        .await
        .map_err(|e| format!("MCP session failed to start: {e}"))?;
    service
        .waiting()
        .await
        .map_err(|e| format!("MCP session ended with an error: {e}"))?;
    Ok(())
}

/// `render-pdf <protocol> <first> <last> <max_px>`: reads a PDF on stdin and prints one JSON
/// [`Rendered`] with the text of every page and pages `first..=last` as PNGs. Run as a child
/// process, so a PDF that crashes, hangs or eats memory takes only the child down.
fn render_pdf(args: &[String]) -> ExitCode {
    let refuse = |why: &str| {
        eprintln!("hydra-mcp render-pdf: {why}");
        ExitCode::FAILURE
    };
    let [version, first, last, max_px] = args else {
        return refuse("usage: hydra-mcp render-pdf 1 <first> <last> <max_px>");
    };
    if version != RENDER_PROTOCOL {
        return refuse(&format!(
            "protocol version {version} is not supported; this hydra-mcp speaks version \
             {RENDER_PROTOCOL}"
        ));
    }
    let (Ok(first), Ok(last), Ok(max_px)) = (first.parse(), last.parse(), max_px.parse()) else {
        return refuse("first, last and max_px must be whole numbers");
    };
    limit_memory();
    let mut pdf = Vec::new();
    if let Err(e) = std::io::stdin()
        .take(MAX_PDF_BYTES as u64 + 1)
        .read_to_end(&mut pdf)
    {
        return refuse(&format!("cannot read the PDF: {e}"));
    }
    if pdf.len() > MAX_PDF_BYTES {
        return refuse("the PDF is too large");
    }
    let out = render(&pdf, first, last, max_px);
    let json = serde_json::to_string(&out).expect("the result serializes");
    let mut stdout = std::io::stdout().lock();
    if let Err(e) = stdout
        .write_all(json.as_bytes())
        .and_then(|()| stdout.flush())
    {
        return refuse(&format!("cannot write the result: {e}"));
    }
    ExitCode::SUCCESS
}

/// Caps the renderer's address space at 2 GiB, best effort.
#[cfg(unix)]
fn limit_memory() {
    const LIMIT: libc::rlim_t = 2 << 30;
    let limit = libc::rlimit {
        rlim_cur: LIMIT,
        rlim_max: LIMIT,
    };
    // SAFETY: setrlimit only reads the struct. Linux enforces it; macOS ignores it.
    unsafe { libc::setrlimit(libc::RLIMIT_AS, &limit) };
}

// ponytail: no memory cap on Windows; a Job Object limit if a hostile PDF ever exhausts memory.
#[cfg(not(unix))]
fn limit_memory() {}

fn render(pdf: &[u8], first: u32, last: u32, max_px: u32) -> Rendered {
    let mut out = Rendered::default();
    match catch_unwind(|| pdf_extract::extract_text_from_mem_by_pages(pdf)) {
        Ok(Ok(pages)) => out.text = Some(pages),
        Ok(Err(e)) => out.text_error = Some(e.to_string()),
        Err(_) => out.text_error = Some("the PDF text reader failed".into()),
    }
    if catch_unwind(AssertUnwindSafe(|| {
        render_pages(pdf, first, last, max_px, &mut out)
    }))
    .is_err()
    {
        out.image_errors.push("the PDF renderer failed".into());
    }
    if out.pages == 0 {
        out.pages = out.text.as_ref().map_or(0, |t| t.len() as u32);
    }
    out
}

fn render_pages(pdf: &[u8], first: u32, last: u32, max_px: u32, out: &mut Rendered) {
    let pdf = match Pdf::new(pdf.to_vec()) {
        Ok(pdf) => pdf,
        Err(e) => {
            out.image_errors
                .push(format!("the PDF can't be opened ({e:?})"));
            return;
        }
    };
    let pages = pdf.pages();
    out.pages = pages.len() as u32;
    let cache = RenderCache::new();
    let settings = InterpreterSettings::default();
    for n in first.max(1)..=last.min(out.pages) {
        let page = &pages[n as usize - 1];
        match catch_unwind(AssertUnwindSafe(|| {
            page_png(page, &cache, &settings, max_px)
        })) {
            Ok(Ok(png)) => out.images.push(PageImage {
                page: n,
                png_base64: base64::engine::general_purpose::STANDARD.encode(png),
            }),
            Ok(Err(e)) => out.image_errors.push(format!("page {n}: {e}")),
            Err(_) => out
                .image_errors
                .push(format!("page {n}: the PDF renderer failed")),
        }
    }
}

/// The page as a 4-bit grayscale PNG with its long edge `max_px`: under half the size of 8-bit
/// and still sharp for datasheet text and drawings.
fn page_png<'a>(
    page: &'a Page<'a>,
    cache: &RenderCache<'a>,
    settings: &InterpreterSettings,
    max_px: u32,
) -> Result<Vec<u8>, String> {
    let (w, h) = page.render_dimensions();
    if w.is_nan() || h.is_nan() || w.min(h) <= 0.0 {
        return Err("the page has no size".into());
    }
    let scale = max_px as f32 / w.max(h);
    let pixmap = hayro::render(
        page,
        cache,
        settings,
        &RenderSettings::default(),
        &PixmapSettings {
            x_scale: scale,
            y_scale: scale,
            bg_color: WHITE,
        },
    );
    let (w, h) = (pixmap.width() as usize, pixmap.height() as usize);
    let row = w.div_ceil(2);
    let mut gray = vec![0u8; row * h];
    // Opaque on white, so premultiplied alpha doesn't matter.
    for (i, p) in pixmap
        .data_as_u8_slice()
        .as_chunks::<4>()
        .0
        .iter()
        .enumerate()
    {
        let luma = (p[0] as u32 * 299 + p[1] as u32 * 587 + p[2] as u32 * 114) / 1000;
        let v = ((luma * 15 + 127) / 255) as u8;
        let (x, y) = (i % w, i / w);
        gray[y * row + x / 2] |= if x % 2 == 0 { v << 4 } else { v };
    }
    let mut png = Vec::new();
    let mut encoder = png::Encoder::new(&mut png, w as u32, h as u32);
    encoder.set_color(png::ColorType::Grayscale);
    encoder.set_depth(png::BitDepth::Four);
    encoder.set_compression(png::Compression::High);
    encoder
        .write_header()
        .and_then(|mut writer| {
            writer.write_image_data(&gray)?;
            writer.finish()
        })
        .map_err(|e| format!("cannot make its image: {e}"))?;
    Ok(png)
}
