//! Parses every `UserGameStatsSchema_*.bin` in a directory and reports on them.
//!
//! This is a correctness check against real Steam data rather than a synthetic
//! fixture, and it doubles as a survey of which schema dialects are actually
//! present on a given machine (the `type` string form versus the older
//! `type_int` form).
//!
//! Usage: cargo run -p sam-vdf --example validate_schemas [-- <stats-dir>]

use std::collections::BTreeMap;
use std::path::PathBuf;

fn main() {
    let dir = std::env::args()
        .nth(1)
        .map(PathBuf::from)
        .unwrap_or_else(|| {
            let home = std::env::var("HOME").expect("HOME must be set");
            PathBuf::from(home).join(".local/share/Steam/appcache/stats")
        });

    let entries = match std::fs::read_dir(&dir) {
        Ok(entries) => entries,
        Err(e) => {
            eprintln!("cannot read {}: {e}", dir.display());
            std::process::exit(1);
        }
    };

    let mut files: Vec<PathBuf> = entries
        .filter_map(|e| e.ok())
        .map(|e| e.path())
        .filter(|p| {
            p.file_name()
                .and_then(|n| n.to_str())
                .is_some_and(|n| n.starts_with("UserGameStatsSchema_") && n.ends_with(".bin"))
        })
        .collect();
    files.sort();

    let mut ok = 0usize;
    let mut failed = Vec::new();
    // How many schemas use each dialect, and the spread of stat type values.
    let mut dialects: BTreeMap<&str, usize> = BTreeMap::new();
    let mut type_values: BTreeMap<String, usize> = BTreeMap::new();
    let mut total_achievements = 0usize;
    let mut total_stats = 0usize;

    for path in &files {
        let data = match std::fs::read(path) {
            Ok(d) => d,
            Err(e) => {
                failed.push((path.clone(), format!("read error: {e}")));
                continue;
            }
        };

        let kv = match sam_vdf::parse(&data) {
            Ok(kv) => kv,
            Err(e) => {
                failed.push((path.clone(), e.to_string()));
                continue;
            }
        };
        ok += 1;

        // The root has exactly one child, keyed by app ID as a decimal string.
        let Some(app) = kv.children.first() else {
            failed.push((path.clone(), "empty root".to_string()));
            continue;
        };

        for stat in &app.get("stats").children {
            let type_node = stat.get("type");
            let type_int_node = stat.get("type_int");

            if type_node.as_str().is_some() {
                *dialects.entry("type (string)").or_default() += 1;
                type_values
                    .entry(type_node.as_string_or("?").to_lowercase())
                    .and_modify(|c| *c += 1)
                    .or_insert(1);
            } else if type_int_node.is_valid() {
                *dialects.entry("type_int").or_default() += 1;
                type_values
                    .entry(format!("int:{}", type_int_node.as_i32_or(-1)))
                    .and_modify(|c| *c += 1)
                    .or_insert(1);
            } else if type_node.is_valid() {
                *dialects.entry("type (numeric)").or_default() += 1;
                type_values
                    .entry(format!("num:{}", type_node.as_i32_or(-1)))
                    .and_modify(|c| *c += 1)
                    .or_insert(1);
            } else {
                *dialects.entry("no type node").or_default() += 1;
            }

            // Achievement blocks nest their entries under repeated `bits` keys.
            let mut achievements_here = 0usize;
            for bits in stat.get_all("bits") {
                achievements_here += bits.children.len();
            }
            if achievements_here > 0 {
                total_achievements += achievements_here;
            } else {
                total_stats += 1;
            }
        }
    }

    println!("scanned  : {}", dir.display());
    println!("files    : {}", files.len());
    println!("parsed   : {ok}");
    println!("failed   : {}", failed.len());
    println!("\nstat blocks by dialect:");
    for (dialect, count) in &dialects {
        println!("  {dialect:<18} {count}");
    }
    println!("\ndistinct type values:");
    for (value, count) in &type_values {
        println!("  {value:<18} {count}");
    }
    println!("\nachievement entries : {total_achievements}");
    println!("numeric stat entries: {total_stats}");

    if !failed.is_empty() {
        println!("\nfailures:");
        for (path, reason) in &failed {
            println!(
                "  {}: {reason}",
                path.file_name().unwrap_or_default().to_string_lossy()
            );
        }
        std::process::exit(1);
    }
}
