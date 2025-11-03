use std::{fs, path::Path, path::PathBuf};

use anyhow::Result;
use clap::{CommandFactory, Parser};
use clap_mangen::Man;

#[derive(Debug, Parser)]
struct Args {
    /// Path to the output manpage for the top-level command
    #[arg(value_name = "FILE", conflicts_with = "dir")]
    out_file: Option<PathBuf>,

    /// Directory to write manpages for the command and all subcommands
    #[arg(long, value_name = "DIR")]
    dir: Option<PathBuf>,
}

fn main() -> Result<()> {
    let args = Args::parse();
    let command = conda_dist::cli::Cli::command()
        .name("conda-dist")
        .bin_name("conda-dist");

    if let Some(dir) = args.dir {
        generate_all_manpages(command, &dir)?;
    } else {
        let out_path = args
            .out_file
            .unwrap_or_else(|| PathBuf::from("docs/src/man/conda-dist.1"));
        generate_single_manpage(command, &out_path)?;
    }

    Ok(())
}

fn generate_single_manpage(command: clap::Command, out_path: &Path) -> Result<()> {
    if let Some(parent) = out_path.parent() {
        fs::create_dir_all(parent)?;
    }

    let mut buffer = Vec::new();
    Man::new(command).render(&mut buffer)?;
    fs::write(out_path, buffer)?;

    eprintln!("Generated man page at {}", out_path.display());
    Ok(())
}

fn generate_all_manpages(command: clap::Command, out_dir: &Path) -> Result<()> {
    fs::create_dir_all(out_dir)?;

    let pages = collect_manpages(command)?;
    for (bin_name, content) in &pages {
        let filename = format!("{}.1", bin_name.replace(' ', "-"));
        fs::write(out_dir.join(filename), content)?;
    }

    eprintln!(
        "Generated {} man pages in {}",
        pages.len(),
        out_dir.display()
    );
    Ok(())
}

fn collect_manpages(command: clap::Command) -> Result<Vec<(String, Vec<u8>)>> {
    let mut pages = Vec::new();
    collect_manpages_inner(&mut pages, command)?;
    Ok(pages)
}

fn collect_manpages_inner(
    pages: &mut Vec<(String, Vec<u8>)>,
    command: clap::Command,
) -> Result<()> {
    let bin_name = command
        .get_bin_name()
        .map(str::to_owned)
        .unwrap_or_else(|| command.get_name().to_owned());

    let mut buffer = Vec::new();
    Man::new(command.clone()).render(&mut buffer)?;
    pages.push((bin_name.clone(), buffer));

    let subcommands: Vec<_> = command
        .get_subcommands()
        .filter(|sub| sub.get_name() != "help")
        .cloned()
        .collect();

    for mut sub in subcommands {
        if sub.get_bin_name().is_none() {
            let sub_name = sub.get_name().to_owned();
            sub = sub.bin_name(format!("{bin_name} {sub_name}"));
        }
        collect_manpages_inner(pages, sub)?;
    }

    Ok(())
}
