use std::{collections::HashMap, path::Path};

use anyhow::{Context, Result};
use rattler_conda_types::{RepoDataRecord, package::AboutJson};
use rattler_package_streaming::seek::read_package_file;
use serde::Serialize;
use tabled::{Table, Tabled, settings::Style};

use crate::bundle::BundleMetadata;

pub fn print_bundle_summary(
    metadata: &BundleMetadata,
    records: &[RepoDataRecord],
    channel_dir: &Path,
) -> Result<()> {
    println!("Bundle: {}", metadata.summary);
    println!("Maintainer: {}", metadata.author);

    if let Some(description) = metadata.description.as_deref() {
        println!();
        print_labeled_block("Description", description);
    }

    if let Some(release_notes) = metadata.release_notes.as_deref() {
        println!();
        print_labeled_block("Release notes", release_notes);
    }

    if !metadata.featured_packages.is_empty() {
        println!();
        println!("Highlighted packages:");
        let index = build_record_index(records);
        for package in &metadata.featured_packages {
            if let Some(record) = index.get(package.name.as_str()) {
                print_highlight(record, channel_dir);
            } else {
                println!("- {} (details unavailable in this bundle)", package.name);
            }
        }
    }

    Ok(())
}

pub fn list_packages_plain(records: &[RepoDataRecord]) {
    let mut rows: Vec<PackageRow> = records
        .iter()
        .map(|record| PackageRow {
            name: record.package_record.name.as_normalized().to_string(),
            version: record.package_record.version.to_string(),
            build: record.package_record.build.to_string(),
            platform: record.package_record.subdir.to_string(),
            license: record
                .package_record
                .license
                .as_deref()
                .unwrap_or("unknown")
                .to_string(),
        })
        .collect();

    rows.sort_by(|a, b| a.name.cmp(&b.name));

    let mut table = Table::new(rows);
    table.with(Style::modern());
    println!("{table}");
}

#[derive(Serialize)]
struct PackageListEntry<'a> {
    name: &'a str,
    version: String,
    build: &'a str,
    platform: &'a str,
    license: Option<&'a str>,
}

pub fn list_packages_json(records: &[RepoDataRecord]) -> Result<()> {
    let mut entries: Vec<_> = records
        .iter()
        .map(|record| PackageListEntry {
            name: record.package_record.name.as_normalized(),
            version: record.package_record.version.to_string(),
            build: record.package_record.build.as_str(),
            platform: record.package_record.subdir.as_str(),
            license: record.package_record.license.as_deref(),
        })
        .collect();

    entries.sort_by(|a, b| a.name.cmp(b.name));

    let json =
        serde_json::to_string_pretty(&entries).context("failed to serialise package list")?;
    println!("{json}");
    Ok(())
}

#[derive(Tabled)]
struct PackageRow {
    #[tabled(rename = "Package")]
    name: String,
    #[tabled(rename = "Version")]
    version: String,
    #[tabled(rename = "Build")]
    build: String,
    #[tabled(rename = "Platform")]
    platform: String,
    #[tabled(rename = "License")]
    license: String,
}

fn build_record_index<'a>(records: &'a [RepoDataRecord]) -> HashMap<&'a str, &'a RepoDataRecord> {
    let mut map = HashMap::new();
    for record in records {
        map.insert(record.package_record.name.as_normalized(), record);
    }
    map
}

fn print_highlight(record: &RepoDataRecord, channel_dir: &Path) {
    println!(
        "- {} {} (build {})",
        record.package_record.name.as_normalized(),
        record.package_record.version,
        record.package_record.build.as_str()
    );
    match load_package_about(channel_dir, record) {
        Some(about) => {
            let mut printed = false;
            if let Some(summary) = about.summary.as_deref() {
                println!("  Summary:");
                print_indented_lines(summary, "    ");
                printed = true;
            }
            if let Some(description) = about
                .description
                .as_deref()
                .filter(|desc| Some(*desc) != about.summary.as_deref())
            {
                if printed {
                    println!();
                }
                println!("  Description:");
                print_indented_lines(description, "    ");
                printed = true;
            }
            if !printed {
                println!("  Summary: (not available)");
            }
        }
        None => println!("  Summary: (metadata unavailable)"),
    }
}

fn load_package_about(channel_dir: &Path, record: &RepoDataRecord) -> Option<AboutJson> {
    let package_path = channel_dir
        .join(&record.package_record.subdir)
        .join(&record.file_name);
    read_package_file::<AboutJson>(&package_path).ok()
}

fn print_labeled_block(label: &str, text: &str) {
    println!("{label}:");
    print_indented_lines(text, "  ");
}

fn print_indented_lines(text: &str, indent: &str) {
    for line in text.trim().lines() {
        if line.trim().is_empty() {
            println!();
        } else {
            println!("{indent}{line}");
        }
    }
}
