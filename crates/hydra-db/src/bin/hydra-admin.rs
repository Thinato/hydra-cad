//! Admin tool: run migrations, import catalogue files, attach datasheets and 3D models.

use std::io::Read;
use std::path::{Path, PathBuf};
use std::process::{Command, ExitCode, Stdio};

use hydra_core::{Catalogue, CatalogueRef, ValveType};
use hydra_db::{
    MIGRATOR, ModelOwner, asset_bytes, attach_datasheet, attach_model, check_schema, connect,
    import_catalogue, load_catalogue, sha256_hex,
};

const USAGE: &str = "usage: hydra-admin <command>   (reads DATABASE_URL)
  migrate                        apply migrations and check the schema
  import <catalogue.json>        import catalogue rows
  attach <key@version> <file.pdf> attach a PDF datasheet to a valve type
  model valve|block <key@version> <file.glb> [--source <file.step>] [options]
  model valve|block <key@version> <file.step|.stp|.zip> [--linear <mm>] [options]
                                 attach a 3D model; a STEP file (or the one STEP file in
                                 a zip) is converted with $HYDRA_STEP2GLTF (default
                                 step2gltf) and kept as source. Options:
    --align auto|none            auto stands a cartridge on its shoulder, axis on +z;
                                 the default for cartridge valve types from STEP
    --rotate x|y|z:<deg>         turn the model about an axis, after aligning; repeatable
    --offset <x,y,z>             then move it, in mm
    --preview <out.glb>          write the model to a file instead of attaching it
  models                         check every stored 3D model
  check                          check the schema version";

#[tokio::main]
async fn main() -> ExitCode {
    let args: Vec<String> = std::env::args().skip(1).collect();
    match run(&args.iter().map(String::as_str).collect::<Vec<_>>()).await {
        Ok(()) => ExitCode::SUCCESS,
        Err(e) => {
            eprintln!("hydra-admin: {e}");
            ExitCode::FAILURE
        }
    }
}

/// List stored symbols this build can't read (written by a newer build).
async fn print_skipped(pool: &sqlx::PgPool) -> Result<(), Box<dyn std::error::Error>> {
    for r in load_catalogue(pool).await?.skipped_symbols {
        println!("warning: stored symbol {r} can't be read by this build; update the app");
    }
    Ok(())
}

async fn run(args: &[&str]) -> Result<(), Box<dyn std::error::Error>> {
    if !matches!(
        args,
        ["migrate" | "check" | "models"]
            | ["import", _]
            | ["attach", _, _]
            | ["model", "valve" | "block", _, _, ..]
    ) {
        return Err(USAGE.into());
    }
    if let ["model", rest @ ..] = args {
        return model(rest).await;
    }
    let pool = connect(&database_url()?).await?;
    match args {
        ["migrate"] => {
            MIGRATOR.run(&pool).await?;
            check_schema(&pool).await?;
            let version = MIGRATOR.iter().map(|m| m.version).max().unwrap_or(0);
            println!("schema migrated to version {version}");
        }
        ["check"] => {
            check_schema(&pool).await?;
            print_skipped(&pool).await?;
            println!("OK");
        }
        ["import", path] => {
            let catalogue: Catalogue = serde_json::from_str(&std::fs::read_to_string(path)?)?;
            let r = import_catalogue(&pool, &catalogue).await?;
            println!(
                "added {} manufacturers, {} symbol defs, {} valve types, {} parts, {} blocks; \
                 {} unchanged",
                r.manufacturers_added,
                r.symbol_defs_added,
                r.valve_types_added,
                r.parts_added,
                r.blocks_added,
                r.unchanged
            );
            print_skipped(&pool).await?;
        }
        ["attach", reference, path] => {
            let reference = CatalogueRef::try_from(reference.to_string())?;
            let sha = attach_datasheet(&pool, &reference, &std::fs::read(path)?).await?;
            println!("{sha}");
        }
        ["models"] => {
            let loaded = load_catalogue(&pool).await?;
            let mut models: Vec<_> = (loaded.valve_models.iter().map(|(r, s)| ("valve", r, s)))
                .chain(loaded.block_models.iter().map(|(r, s)| ("block", r, s)))
                .collect();
            models.sort();
            let mut failed = 0;
            for (kind, r, sha) in &models {
                let checked = match asset_bytes(&pool, sha).await? {
                    None => Err(format!("asset {sha} is missing")),
                    Some(bytes) if sha256_hex(&bytes) != **sha => {
                        Err(format!("asset {sha} does not match its hash"))
                    }
                    Some(bytes) => hydra_mesh::load_glb(&bytes),
                };
                match checked {
                    Ok(mesh) => {
                        let (lo, hi) = mesh.bounds();
                        println!(
                            "ok   {kind} {r}  {} triangles  {:.0}x{:.0}x{:.0} mm",
                            mesh.indices.len() / 3,
                            hi[0] - lo[0],
                            hi[1] - lo[1],
                            hi[2] - lo[2]
                        );
                    }
                    Err(e) => {
                        failed += 1;
                        println!("FAIL {kind} {r}  {e}");
                    }
                }
            }
            if failed > 0 {
                return Err(format!("{failed} of {} models failed", models.len()).into());
            }
        }
        _ => unreachable!(),
    }
    Ok(())
}

fn database_url() -> Result<String, &'static str> {
    std::env::var("DATABASE_URL").map_err(|_| "DATABASE_URL is not set")
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Input {
    Glb,
    Step,
    Zip,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Align {
    Auto,
    None,
}

/// `model` arguments, parsed.
#[derive(Debug, PartialEq)]
struct ModelArgs<'a> {
    owner: ModelOwner,
    path: &'a str,
    input: Input,
    source: Option<&'a str>,
    linear: Option<&'a str>,
    align: Option<Align>,
    /// Axis (x, y or z) and degrees, in order.
    rotate: Vec<(char, f64)>,
    offset: Option<[f64; 3]>,
    preview: Option<&'a str>,
}

/// Parses `valve|block <key@version> <file> [flags]`.
fn parse_model<'a>(args: &[&'a str]) -> Result<ModelArgs<'a>, String> {
    let [kind, reference, path, flags @ ..] = args else {
        return Err(USAGE.into());
    };
    let reference = CatalogueRef::try_from(reference.to_string())?;
    let owner = match *kind {
        "valve" => ModelOwner::ValveType(reference),
        "block" => ModelOwner::Block(reference),
        _ => return Err(USAGE.into()),
    };
    let lower = path.to_ascii_lowercase();
    let input = if lower.ends_with(".step") || lower.ends_with(".stp") {
        Input::Step
    } else if lower.ends_with(".zip") {
        Input::Zip
    } else {
        Input::Glb
    };
    let mut m = ModelArgs {
        owner,
        path,
        input,
        source: None,
        linear: None,
        align: None,
        rotate: Vec::new(),
        offset: None,
        preview: None,
    };
    for pair in flags.chunks(2) {
        match *pair {
            ["--source", f] => m.source = Some(f),
            ["--linear", mm] => m.linear = Some(mm),
            ["--align", "auto"] => m.align = Some(Align::Auto),
            ["--align", "none"] => m.align = Some(Align::None),
            ["--rotate", r] => {
                let turn = r.split_once(':').and_then(|(axis, deg)| {
                    let axis = match axis {
                        "x" | "y" | "z" => axis.chars().next()?,
                        _ => return None,
                    };
                    Some((axis, deg.parse::<f64>().ok().filter(|d| d.is_finite())?))
                });
                m.rotate.push(
                    turn.ok_or(format!("--rotate {r}: give an axis and degrees, e.g. z:90"))?,
                );
            }
            ["--offset", xyz] => {
                let v: Vec<f64> = xyz
                    .split(',')
                    .map(|c| c.trim().parse::<f64>().ok().filter(|c| c.is_finite()))
                    .collect::<Option<_>>()
                    .unwrap_or_default();
                let v: [f64; 3] = v.try_into().map_err(|_| {
                    format!("--offset {xyz}: give three numbers in mm, e.g. 0,0,-5")
                })?;
                m.offset = Some(v);
            }
            ["--preview", out] => m.preview = Some(out),
            _ => return Err(USAGE.into()),
        }
    }
    if m.source.is_some() && input != Input::Glb {
        return Err("--source is only for a glb file; a STEP file is its own source".into());
    }
    if m.linear.is_some() && input == Input::Glb {
        return Err("--linear is only for a STEP or zip file".into());
    }
    Ok(m)
}

/// Only cartridges converted from STEP are stood up by default: a glb, a block or a
/// subplate valve is taken as already in place.
fn default_align(input: Input, owner: &ModelOwner, catalogue: &Catalogue) -> Align {
    match owner {
        ModelOwner::ValveType(r)
            if input != Input::Glb
                && catalogue.valve_type(r).is_some_and(ValveType::is_cartridge) =>
        {
            Align::Auto
        }
        _ => Align::None,
    }
}

async fn model(args: &[&str]) -> Result<(), Box<dyn std::error::Error>> {
    let m = parse_model(args)?;
    // The catalogue gives the default alignment; a preview with --align needs no database.
    let (pool, align) = match m.align {
        Some(align) if m.preview.is_some() => (None, align),
        _ => {
            let pool = connect(&database_url()?).await?;
            let align = match m.align {
                Some(align) => align,
                None => default_align(m.input, &m.owner, &load_catalogue(&pool).await?.catalogue),
            };
            (Some(pool), align)
        }
    };
    let (glb, step) = match m.input {
        Input::Glb => (
            std::fs::read(m.path)?,
            m.source.map(std::fs::read).transpose()?,
        ),
        Input::Step => (
            step_to_glb(Path::new(m.path), m.linear)?,
            Some(std::fs::read(m.path)?),
        ),
        Input::Zip => {
            let step = zip_step(m.path, ZIP_STEP_CAP)?;
            let tmp = temp_file("step")?;
            let glb = std::fs::write(&tmp, &step)
                .map_err(|e| format!("can't write {}: {e}", tmp.display()))
                .and_then(|()| step_to_glb(&tmp, m.linear));
            let _ = std::fs::remove_file(&tmp);
            (glb?, Some(step))
        }
    };
    let source = Path::new(m.path)
        .file_name()
        .map_or(m.path.into(), |n| n.to_string_lossy());
    let glb = align_model(glb, align, &m.rotate, m.offset, &source)?;
    let (lo, hi) = hydra_mesh::load_glb(&glb)?.bounds();
    // On stderr: stdout carries only the hash.
    eprintln!(
        "{:.1} x {:.1} x {:.1} mm, z {:.1}..{:.1}",
        hi[0] - lo[0],
        hi[1] - lo[1],
        hi[2] - lo[2],
        lo[2],
        hi[2]
    );
    if let Some(out) = m.preview {
        std::fs::write(out, &glb)?;
        println!("wrote {out}");
        return Ok(());
    }
    let pool = pool.expect("connected unless previewing");
    println!(
        "{}",
        attach_model(&pool, &m.owner, &glb, step.as_deref()).await?
    );
    Ok(())
}

/// The glb to store: unchanged when nothing moves it, else wrapped in one root node that
/// aligns it and records how.
fn align_model(
    glb: Vec<u8>,
    align: Align,
    rotate: &[(char, f64)],
    offset: Option<[f64; 3]>,
    source: &str,
) -> Result<Vec<u8>, String> {
    if align == Align::None && rotate.is_empty() && offset.is_none() {
        return Ok(glb);
    }
    let guess = match align {
        Align::Auto => Some(hydra_mesh::guess_alignment(&hydra_mesh::load_glb(&glb)?)),
        Align::None => None,
    };
    let mut m = guess.unwrap_or([
        [1.0, 0.0, 0.0, 0.0],
        [0.0, 1.0, 0.0, 0.0],
        [0.0, 0.0, 1.0, 0.0],
        [0.0, 0.0, 0.0, 1.0],
    ]);
    for &(axis, deg) in rotate {
        // Turning every column turns the whole transform.
        let (s, c) = deg.to_radians().sin_cos();
        let (i, j) = match axis {
            'x' => (1, 2),
            'y' => (2, 0),
            _ => (0, 1),
        };
        for col in &mut m {
            (col[i], col[j]) = (c * col[i] - s * col[j], s * col[i] + c * col[j]);
        }
    }
    if let Some(offset) = offset {
        (0..3).for_each(|k| m[3][k] += offset[k]);
    }
    let note = serde_json::json!({
        "guess": guess,
        "rotate": rotate.iter().map(|(a, d)| format!("{a}:{d}")).collect::<Vec<_>>(),
        "offset": offset,
        "source": source,
    });
    hydra_mesh::set_root_transform(&glb, m, note)
}

/// The most a STEP file taken from a zip may be.
const ZIP_STEP_CAP: u64 = 256 << 20;

/// The one STEP file in a zip, read with tar (bsdtar on macOS and Windows 10+).
fn zip_step(zip: &str, cap: u64) -> Result<Vec<u8>, String> {
    let unreadable =
        |e: String| format!("can't read {zip} with tar ({e}); unzip it and pass the .step");
    let listing = Command::new("tar")
        .args(["-tf", zip])
        .output()
        .map_err(|e| unreadable(e.to_string()))?;
    if !listing.status.success() {
        return Err(unreadable(
            String::from_utf8_lossy(&listing.stderr).trim().into(),
        ));
    }
    let entry = zip_step_entry(&String::from_utf8_lossy(&listing.stdout))?;
    let mut tar = Command::new("tar")
        .args(["-xOf", zip, "--", &entry])
        .stdout(Stdio::piped())
        .spawn()
        .map_err(|e| unreadable(e.to_string()))?;
    let read = read_capped(tar.stdout.take().expect("piped"), cap);
    if read.is_err() {
        let _ = tar.kill();
    }
    let status = tar.wait().map_err(|e| unreadable(e.to_string()))?;
    let step = read?;
    if !status.success() {
        return Err(unreadable(format!("extracting {entry}: {status}")));
    }
    Ok(step)
}

/// The zip entry to convert: its only STEP file, from names that stay inside the zip.
fn zip_step_entry(listing: &str) -> Result<String, String> {
    let names: Vec<&str> = listing.lines().filter(|n| !n.is_empty()).collect();
    let drive =
        |n: &str| n.as_bytes().get(1) == Some(&b':') && n.as_bytes()[0].is_ascii_alphabetic();
    if let Some(bad) = names
        .iter()
        .find(|n| n.contains("..") || n.starts_with(['/', '\\']) || drive(n))
    {
        return Err(format!(
            "the zip holds an unsafe file name {bad:?}; refusing it"
        ));
    }
    let steps: Vec<&str> = names
        .into_iter()
        // macOS Finder adds a resource-fork copy of every file under __MACOSX/.
        .filter(|n| !n.starts_with("__MACOSX/"))
        .filter(|n| {
            let n = n.to_ascii_lowercase();
            n.ends_with(".step") || n.ends_with(".stp")
        })
        .collect();
    match steps[..] {
        // tar reads the name back as a pattern, which could match several entries.
        [one] if one.contains(['*', '?', '[', '\\']) => Err(format!(
            "the STEP file's name {one:?} has wildcard characters; unzip it and pass the file"
        )),
        [one] => Ok(one.into()),
        [] => Err("the zip holds no .step or .stp file".into()),
        _ => Err(format!(
            "the zip holds {} STEP files ({}); unzip it and pass the one to use",
            steps.len(),
            steps.join(", ")
        )),
    }
}

/// Reads at most `cap` bytes, refusing more.
fn read_capped(r: impl Read, cap: u64) -> Result<Vec<u8>, String> {
    let mut bytes = Vec::new();
    r.take(cap + 1)
        .read_to_end(&mut bytes)
        .map_err(|e| format!("can't read the STEP file from the zip: {e}"))?;
    if bytes.len() as u64 > cap {
        return Err(format!(
            "the STEP file in the zip is larger than {} MB",
            cap as f64 / (1 << 20) as f64
        ));
    }
    Ok(bytes)
}

/// A new empty temp file. Created exclusively, so a file or symlink planted at that name in
/// a shared temp directory is refused rather than written through.
fn temp_file(ext: &str) -> Result<PathBuf, String> {
    let nanos = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map_or(0, |d| d.as_nanos());
    let tmp = std::env::temp_dir().join(format!("hydra-{}-{nanos}.{ext}", std::process::id()));
    std::fs::File::create_new(&tmp).map_err(|e| format!("can't create {}: {e}", tmp.display()))?;
    Ok(tmp)
}

/// Convert a STEP file to glb bytes with the external converter.
fn step_to_glb(step: &Path, linear: Option<&str>) -> Result<Vec<u8>, String> {
    let prog = std::env::var("HYDRA_STEP2GLTF").unwrap_or_else(|_| "step2gltf".into());
    let tmp = temp_file("glb")?;
    let mut cmd = Command::new(&prog);
    // The converter's summary goes to stderr, so stdout carries only the hash.
    cmd.arg(step).arg(&tmp).stdout(std::io::stderr());
    if let Some(mm) = linear {
        cmd.args(["--linear", mm]);
    }
    let result = match cmd.status() {
        Err(e) => Err(format!(
            "can't run the STEP converter {prog:?} ({e}); set HYDRA_STEP2GLTF to its path"
        )),
        Ok(status) if !status.success() => Err(format!(
            "the STEP converter {prog:?} failed ({status}); check the file, or set \
             HYDRA_STEP2GLTF to another converter"
        )),
        Ok(_) => std::fs::read(&tmp).map_err(|e| {
            format!("the STEP converter {prog:?} wrote no model ({e}); check HYDRA_STEP2GLTF")
        }),
    };
    let _ = std::fs::remove_file(&tmp);
    result
}

#[cfg(test)]
mod tests {
    use super::*;

    const ROOT: &str = concat!(env!("CARGO_MANIFEST_DIR"), "/../..");

    fn catalogue() -> Catalogue {
        let text = std::fs::read_to_string(format!("{ROOT}/fixtures/catalogue.json")).unwrap();
        serde_json::from_str(&text).unwrap()
    }

    fn model_file(name: &str) -> Vec<u8> {
        std::fs::read(format!("{ROOT}/fixtures/models/{name}")).unwrap()
    }

    fn valve(r: &str) -> ModelOwner {
        ModelOwner::ValveType(r.to_string().try_into().unwrap())
    }

    #[test]
    fn model_arguments_parse() {
        let m = parse_model(&["block", "blk-demo@1", "b.glb", "--source", "b.step"]).unwrap();
        assert_eq!(m.owner, ModelOwner::Block(CatalogueRef::new("blk-demo", 1)));
        assert_eq!(
            (m.input, m.source, m.align),
            (Input::Glb, Some("b.step"), None)
        );

        let m = parse_model(&[
            "valve",
            "vt-x@2",
            "c.STP",
            "--linear",
            "0.5",
            "--align",
            "auto",
            "--rotate",
            "x:90",
            "--rotate",
            "z:-45",
            "--offset",
            "1, 2,-3.5",
            "--preview",
            "out.glb",
        ])
        .unwrap();
        assert_eq!(m.owner, valve("vt-x@2"));
        assert_eq!(
            (m.input, m.linear, m.align),
            (Input::Step, Some("0.5"), Some(Align::Auto))
        );
        assert_eq!(m.rotate, [('x', 90.0), ('z', -45.0)]);
        assert_eq!(m.offset, Some([1.0, 2.0, -3.5]));
        assert_eq!(m.preview, Some("out.glb"));
        assert_eq!(
            parse_model(&["valve", "vt-x@2", "c.zip"]).unwrap().input,
            Input::Zip
        );

        let err = |args: &[&str]| parse_model(args).unwrap_err();
        assert!(err(&["valve", "vt-x@2", "c.step", "--source", "c.step"]).contains("--source"));
        assert!(err(&["valve", "vt-x@2", "c.glb", "--linear", "1"]).contains("--linear"));
        assert!(err(&["valve", "vt-x@2", "c.glb", "--rotate", "w:90"]).contains("z:90"));
        assert!(err(&["valve", "vt-x@2", "c.glb", "--rotate", "z:NaN"]).contains("z:90"));
        assert!(err(&["valve", "vt-x@2", "c.glb", "--offset", "1,2"]).contains("three"));
        for usage in [
            &["valve", "vt-x@2", "c.glb", "--align", "maybe"][..],
            &["valve", "vt-x@2", "c.glb", "--offset"],
            &["pump", "vt-x@2", "c.glb"],
        ] {
            assert_eq!(err(usage), USAGE);
        }
    }

    #[test]
    fn only_cartridges_from_step_are_aligned_by_default() {
        let c = catalogue();
        let cartridge = valve("vt-demo-cartridge-relief@1");
        assert_eq!(default_align(Input::Step, &cartridge, &c), Align::Auto);
        assert_eq!(default_align(Input::Zip, &cartridge, &c), Align::Auto);
        assert_eq!(default_align(Input::Glb, &cartridge, &c), Align::None);
        assert_eq!(
            default_align(Input::Step, &valve("vt-demo-4-2@1"), &c),
            Align::None
        );
        assert_eq!(
            default_align(Input::Step, &valve("vt-missing@1"), &c),
            Align::None
        );
        let block = ModelOwner::Block(CatalogueRef::new("blk-demo", 2));
        assert_eq!(default_align(Input::Step, &block, &c), Align::None);
    }

    #[test]
    fn zip_entries_must_be_one_safe_step_file() {
        assert_eq!(
            zip_step_entry(
                "docs/\ndocs/readme.txt\nmodel/Valve.STEP\n__MACOSX/model/._Valve.STEP\n"
            ),
            Ok("model/Valve.STEP".into())
        );
        for bad in [
            "../evil.step",
            "a/../../b.stp",
            "/etc/x.step",
            "\\x.step",
            "C:x.step",
        ] {
            let e = zip_step_entry(&format!("ok.step\n{bad}\n")).unwrap_err();
            assert!(e.contains("unsafe"), "{bad}: {e}");
        }
        let e = zip_step_entry("a.step\nb/c.stp\n").unwrap_err();
        assert!(e.contains("a.step, b/c.stp"), "{e}");
        assert_eq!(
            zip_step_entry("readme.txt\n").unwrap_err(),
            "the zip holds no .step or .stp file"
        );
        for wild in ["v*.step", "v?.stp", "v[1].step"] {
            let e = zip_step_entry(&format!("{wild}\n")).unwrap_err();
            assert!(e.contains("wildcard"), "{wild}: {e}");
        }
        assert_eq!(read_capped(&b"0123456789"[..], 10).unwrap().len(), 10);
        assert!(
            read_capped(&b"0123456789!"[..], 10)
                .unwrap_err()
                .contains("larger than")
        );
    }

    #[test]
    fn nothing_to_move_keeps_the_glb_bytes() {
        let glb = model_file("demo-valve-cetop3.glb");
        assert_eq!(
            align_model(glb.clone(), Align::None, &[], None, "v.glb").unwrap(),
            glb
        );
        // z:90 then 10 mm along x: (x, y, z) -> (10 - y, x, z), the valve spanning
        // x -49.5..90, y -6..38.
        let turned = [('z', 90.0)];
        let moved = align_model(glb, Align::None, &turned, Some([10.0, 0.0, 0.0]), "v.glb");
        let (lo, hi) = hydra_mesh::load_glb(&moved.unwrap()).unwrap().bounds();
        let near = |a: f32, b: f32| (a - b).abs() < 0.5;
        assert!(near(lo[0], -28.0) && near(hi[0], 16.0), "{lo:?}..{hi:?}");
        assert!(near(lo[1], -49.5) && near(hi[1], 90.0), "{lo:?}..{hi:?}");
        // The demo cartridge already stands on its seat: auto leaves it there.
        let auto = align_model(
            model_file("demo-cartridge.glb"),
            Align::Auto,
            &[],
            None,
            "c.step",
        );
        let (lo, hi) = hydra_mesh::load_glb(&auto.unwrap()).unwrap().bounds();
        assert!(near(lo[2], -55.0) && near(hi[2], 55.0), "{lo:?}..{hi:?}");
    }
}
