//! Verifies this machine's `steamclient.so` against what the crate expects.
//!
//! **This example is strictly read-only.** It never writes a stat, never
//! unlocks an achievement and never calls `StoreStats`.
//!
//! It answers four questions that cannot be settled by reading headers:
//!
//! 1. Can a plain native process outside the Steam Runtime talk to the client?
//! 2. Do the vtable slot indices resolve into `steamclient.so`?
//! 3. Which way round is the `GetStat` int/float overload pair?
//! 4. Can one process switch between apps, or does `SteamAppId` latch?
//!
//! Run with Steam running and signed in:
//!
//! ```text
//! cargo run -p sam-steam --example probe_abi
//! cargo run -p sam-steam --example probe_abi -- 620   # force an app id
//! ```
//!
//! Note the ordering below: the target app is chosen from the *cached schema
//! files on disk*, before Steam is loaded at all. That is deliberate.
//! `steamclient.so` reads `SteamAppId` when it initialises, so the variable
//! has to be set before `dlopen`. Picking the app from a live session first
//! would be too late, and every stats call would target app 0.

use std::path::Path;
use std::time::{Duration, Instant};

use sam_steam::{CallbackEvent, CallbackPump, GameSchema, Session, Steam, SteamPaths, UserStats};

fn main() {
    if let Err(e) = run() {
        eprintln!("\nprobe failed: {e}");
        std::process::exit(1);
    }
}

fn run() -> Result<(), sam_steam::Error> {
    let forced_app: Option<u32> = std::env::args().nth(1).and_then(|a| a.parse().ok());

    // ---- 1. Locate Steam, without loading anything -----------------------
    println!("== installation ==");
    let paths = SteamPaths::discover()?;
    println!("steam root : {}", paths.root().display());
    println!("client lib : {}", paths.client_library().display());

    let cached = paths.apps_with_cached_schema();
    println!("cached schemas: {}", cached.len());

    // ---- 2. Choose a target from disk alone ------------------------------
    // The overload probe needs an app with a numeric stat of known type.
    let app_id = match forced_app.or_else(|| pick_candidate(&paths, &cached)) {
        Some(id) => id,
        None => {
            println!("\nno cached schema has a numeric stat to probe with.");
            println!("pass an app id explicitly to override.");
            return Ok(());
        }
    };

    // Must be set before `Steam::load`, which dlopens the client.
    std::env::set_var("SteamAppId", app_id.to_string());

    // ---- 3. Connect ------------------------------------------------------
    println!("\n== session (app {app_id}) ==");
    let steam = Steam::load_from(paths)?;
    let session = steam.connect(Some(app_id))?;

    let steam_id = session.steam_id();
    println!("steam id   : {steam_id}");
    // Individual account IDs live in the 7656119... range. A wildly different
    // value would mean CSteamID is not returned in a register as assumed.
    if (76561197960265728..76561202255233024).contains(&steam_id) {
        println!("             ^ plausible individual account id");
    } else {
        println!("             ^ WARNING: outside the expected range");
    }

    let language = match session.current_game_language() {
        lang if lang.is_empty() => "english".to_string(),
        lang => lang,
    };
    println!("language   : {language}");
    println!("GetAppID   : {}", session.current_app_id());
    println!("owns it    : {}", session.owns_app(app_id));

    let schema =
        GameSchema::load(&steam.paths().schema_path(app_id), app_id, &language).unwrap_or_default();
    let name = session
        .app_name(app_id)
        .unwrap_or_else(|| app_id.to_string());
    println!(
        "target     : {name} — {} achievements, {} stats",
        schema.achievements.len(),
        schema.stats.len()
    );

    let mut stats = UserStats::new(&session)?;
    println!("vtable     : all probed slots resolve into steamclient.so");

    println!("\nrequesting user stats...");
    stats.request_user_stats(steam_id);
    match wait_for_stats(&session, Duration::from_secs(10)) {
        Some(1) => println!("UserStatsReceived: ok"),
        Some(result) => println!("UserStatsReceived: EResult {result} (not ok)"),
        None => println!("UserStatsReceived: timed out"),
    }

    // ---- 4. Cross-check the achievement count ----------------------------
    let reported = stats.num_achievements();
    println!("\n== vtable cross-check ==");
    println!("GetNumAchievements : {reported}");
    println!("schema achievements: {}", schema.achievements.len());
    if reported as usize == schema.achievements.len() {
        println!("                     ^ match — base vtable alignment confirmed");
    } else {
        println!("                     ^ MISMATCH — indices may be shifted");
    }

    if let Some(first) = stats.achievement_name(0) {
        println!("GetAchievementName(0): {first}");
        println!(
            "  present in schema  : {}",
            schema.achievement(&first).is_some()
        );
    }

    // ---- 5. The overload question ----------------------------------------
    println!("\n== GetStat overload order ==");
    let resolution =
        stats.resolve_overload_order(&schema.integer_stat_names(), &schema.float_stat_names());
    println!("order      : {:?}", resolution.order);
    println!("confidence : {:?}", resolution.confidence);
    if let Some(probe) = &resolution.probe_stat {
        println!("proved with: {probe}");
    }

    println!("\nsample stat reads (read-only):");
    for def in schema.stats.iter().take(6) {
        if def.is_integer() {
            println!("  {:<34} i32 = {:?}", def.id, stats.stat_i32(&def.id));
        } else {
            println!("  {:<34} f32 = {:?}", def.id, stats.stat_f32(&def.id));
        }
    }

    println!("\nsample achievement reads (read-only):");
    for def in schema.achievements.iter().take(6) {
        match stats.achievement_and_unlock_time(&def.id) {
            Some((unlocked, when)) => println!(
                "  [{}] {:<40} unlock_time={}",
                if unlocked { 'x' } else { ' ' },
                truncate(&def.name, 40),
                when
            ),
            None => println!("  [?] {:<40} (no value)", truncate(&def.name, 40)),
        }
    }

    // ---- 6. Confirm the SteamAppId latch ---------------------------------
    // The UI depends on this being true, so it is worth re-checking on every
    // machine rather than taking it on trust.
    println!("\n== app switching ==");
    let other = cached
        .iter()
        .copied()
        .find(|&id| id != app_id && session.owns_app(id));
    match other {
        Some(other_id) => {
            drop(session);
            match steam.connect(Some(other_id)) {
                Ok(next) => {
                    let reported = next.current_app_id();
                    println!("asked for {other_id}, GetAppID reports {reported}");
                    if reported == other_id {
                        println!("  ^ a single process CAN retarget between apps");
                    } else {
                        println!("  ^ SteamAppId latched, as expected: one process per app");
                    }
                }
                // The expected outcome: `Session::open` compares GetAppID
                // against the request and refuses the mismatch.
                Err(e) => println!(
                    "re-connect refused, as expected: {e}\n  \
                     ^ SteamAppId latched: one process per app"
                ),
            }
        }
        None => println!("no second owned app available to test with"),
    }

    println!("\nprobe complete. nothing was written.");
    Ok(())
}

/// First cached schema that declares a numeric stat, which is what the
/// overload probe needs. Uses only files on disk, so it can run before Steam
/// is loaded.
fn pick_candidate(paths: &SteamPaths, cached: &[u32]) -> Option<u32> {
    let mut fallback = None;

    for &app_id in cached {
        let path: &Path = &paths.schema_path(app_id);
        let Ok(schema) = GameSchema::load(path, app_id, "english") else {
            continue;
        };
        if schema.achievements.is_empty() {
            continue;
        }
        if !schema.integer_stat_names().is_empty() || !schema.float_stat_names().is_empty() {
            return Some(app_id);
        }
        fallback.get_or_insert(app_id);
    }

    fallback
}

/// Pump callbacks until `UserStatsReceived` arrives, returning its `EResult`.
fn wait_for_stats(session: &Session, timeout: Duration) -> Option<i32> {
    let pump = CallbackPump::new(session);
    let deadline = Instant::now() + timeout;

    while Instant::now() < deadline {
        for event in pump.poll() {
            if let CallbackEvent::UserStatsReceived { result, .. } = event {
                return Some(result);
            }
        }
        std::thread::sleep(Duration::from_millis(25));
    }
    None
}

fn truncate(text: &str, max: usize) -> String {
    if text.chars().count() <= max {
        return text.to_string();
    }
    text.chars().take(max.saturating_sub(1)).collect::<String>() + "…"
}
