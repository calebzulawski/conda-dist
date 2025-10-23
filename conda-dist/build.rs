use std::env;
use std::error::Error;
use std::fs::{self, File};
use std::io::Write;
use std::path::PathBuf;

fn main() -> Result<(), Box<dyn Error>> {
    println!("cargo:rerun-if-changed=build.rs");
    println!("cargo:rerun-if-changed=installers");

    let out_dir = PathBuf::from(env::var("OUT_DIR")?);
    let manifest_dir = PathBuf::from(env::var("CARGO_MANIFEST_DIR")?);
    let installers_dir = manifest_dir.join("installers");
    let installers_path = out_dir.join("installers.rs");

    let mut entries = Vec::new();
    for entry in fs::read_dir(&installers_dir)? {
        let entry = entry?;
        if !entry.file_type()?.is_file() {
            continue;
        }
        let path = entry.path();
        let name = entry
            .file_name()
            .into_string()
            .map_err(|_| "installer file name is not valid UTF-8")?;
        entries.push((name, path));
    }

    entries.sort_by(|a, b| a.0.cmp(&b.0));

    let mut installers_source = File::create(&installers_path)?;
    writeln!(
        installers_source,
        "pub const INSTALLERS: &[(&str, &[u8])] = &["
    )?;
    for (name, path) in entries {
        writeln!(
            installers_source,
            r#"    ("{name}", include_bytes!("{}")),"#,
            path.display()
        )?;
    }
    writeln!(installers_source, "];")?;

    Ok(())
}
