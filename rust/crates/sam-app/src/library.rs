//! Working out which games to show.
//!
//! The Windows picker downloads a master list of every app SAM knows about
//! (`https://gib.me/sam/games.xml`) and asks Steam about each one. That works,
//! but it needs the network before showing anything and depends on a
//! third-party host staying up.
//!
//! On Linux there is a better first source: the local Steam installation
//! already knows which apps have achievements, because it caches a schema for
//! each one. That is instant, offline, and every entry is guaranteed to have
//! something worth editing. The remote list is kept as a fallback, since it
//! also covers owned games that have never been launched and so have no
//! cached schema yet.

use std::collections::BTreeSet;
use std::path::{Path, PathBuf};
use std::sync::mpsc::Receiver;
use std::time::Duration;

use sam_steam::{Session, SteamPaths};

/// Where a candidate app ID came from.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Source {
    /// Steam has a cached achievement schema for it.
    CachedSchema,
    /// It is installed, per an `appmanifest_*.acf`.
    Installed,
    /// It came from the remote master list.
    Remote,
}

/// A game to show in the picker.
#[derive(Debug, Clone)]
pub struct GameEntry {
    pub app_id: u32,
    pub name: String,
    pub capsule_url: Option<String>,
    pub kind: String,
    pub source: Source,
    pub has_schema: bool,
}

/// App IDs worth checking, gathered without any network access.
///
/// Combines apps with a cached achievement schema and apps that are merely
/// installed. The second set catches a game installed but never launched,
/// where opening the editor will at least explain why it is empty.
pub fn local_candidates(paths: &SteamPaths) -> Vec<u32> {
    let mut ids: BTreeSet<u32> = paths.apps_with_cached_schema().into_iter().collect();

    for library in library_folders(paths.root()) {
        ids.extend(installed_app_ids(&library));
    }

    ids.into_iter().collect()
}

/// Every Steam library directory, including those on other drives.
///
/// `libraryfolders.vdf` is *text* VDF rather than the binary format the schema
/// files use, so rather than pull in a second parser we scan for the quoted
/// values that follow each `"path"` key. That is all we need from it.
fn library_folders(root: &Path) -> Vec<PathBuf> {
    let mut folders = vec![root.join("steamapps")];

    let manifest = root.join("steamapps").join("libraryfolders.vdf");
    if let Ok(text) = std::fs::read_to_string(&manifest) {
        for path in quoted_values_for_key(&text, "path") {
            let candidate = PathBuf::from(path).join("steamapps");
            if candidate.is_dir() && !folders.contains(&candidate) {
                folders.push(candidate);
            }
        }
    }

    folders
}

/// Values of `"<key>"  "<value>"` pairs in a text VDF document.
fn quoted_values_for_key(text: &str, key: &str) -> Vec<String> {
    let mut out = Vec::new();

    for line in text.lines() {
        let mut parts = line.split('"').skip(1).step_by(2);
        let (Some(found_key), Some(value)) = (parts.next(), parts.next()) else {
            continue;
        };
        if found_key.eq_ignore_ascii_case(key) {
            out.push(value.to_string());
        }
    }

    out
}

/// App IDs from `appmanifest_<id>.acf` names. The filename carries the ID, so
/// nothing inside the file needs parsing.
fn installed_app_ids(steamapps: &Path) -> Vec<u32> {
    let Ok(entries) = std::fs::read_dir(steamapps) else {
        return Vec::new();
    };

    entries
        .filter_map(|e| e.ok())
        .filter_map(|e| {
            let name = e.file_name();
            let name = name.to_str()?;
            name.strip_prefix("appmanifest_")?
                .strip_suffix(".acf")?
                .parse()
                .ok()
        })
        .collect()
}

/// An entry from the remote master list.
#[derive(Debug, Clone)]
pub struct RemoteApp {
    pub app_id: u32,
    pub kind: String,
}

/// Fetch the master list on a worker thread.
///
/// The result arrives on the returned channel. Ownership filtering cannot
/// happen here, because talking to Steam requires the session, which lives on
/// the UI thread.
pub fn spawn_remote_fetch() -> Receiver<Result<Vec<RemoteApp>, String>> {
    let (tx, rx) = std::sync::mpsc::channel();

    std::thread::Builder::new()
        .name("sam-gamelist".into())
        .spawn(move || {
            let _ = tx.send(fetch_remote_list());
        })
        .expect("spawning the game-list thread should not fail");

    rx
}

fn fetch_remote_list() -> Result<Vec<RemoteApp>, String> {
    let agent: ureq::Agent = ureq::Agent::config_builder()
        .timeout_global(Some(Duration::from_secs(30)))
        .user_agent("sam-linux")
        .build()
        .into();

    let mut response = agent
        .get("https://gib.me/sam/games.xml")
        .call()
        .map_err(|e| format!("could not fetch the game list: {e}"))?;

    let body = response
        .body_mut()
        .with_config()
        .limit(32 * 1024 * 1024)
        .read_to_string()
        .map_err(|e| format!("could not read the game list: {e}"))?;

    Ok(parse_games_xml(&body))
}

/// Extract `<game type="...">12345</game>` entries.
///
/// The document is a flat list of one element type, so a scan is clearer and
/// lighter than pulling in an XML parser.
fn parse_games_xml(xml: &str) -> Vec<RemoteApp> {
    let mut out = Vec::new();
    let mut rest = xml;

    while let Some(start) = rest.find("<game") {
        let after = &rest[start + "<game".len()..];

        // The document's root element is `<games>`, which shares this prefix.
        // Require a real tag boundary so the root is not mistaken for an
        // entry, which would consume the first game along with it.
        if !matches!(
            after.as_bytes().first(),
            Some(b'>') | Some(b' ') | Some(b'\t') | Some(b'\n') | Some(b'\r') | Some(b'/')
        ) {
            rest = after;
            continue;
        }
        rest = after;

        let Some(open_end) = rest.find('>') else {
            break;
        };
        let attributes = &rest[..open_end];
        rest = &rest[open_end + 1..];

        let Some(close) = rest.find("</game>") else {
            break;
        };
        let body = rest[..close].trim();
        rest = &rest[close + "</game>".len()..];

        let Ok(app_id) = body.parse::<u32>() else {
            continue;
        };

        let kind = attribute_value(attributes, "type").unwrap_or_else(|| "normal".to_string());
        out.push(RemoteApp { app_id, kind });
    }

    out
}

fn attribute_value(attributes: &str, name: &str) -> Option<String> {
    let key = format!("{name}=\"");
    let start = attributes.find(&key)? + key.len();
    let end = attributes[start..].find('"')? + start;
    Some(attributes[start..end].to_string())
}

/// Build a display entry for an owned app.
///
/// `name` may be absent when Steam has not cached metadata yet; the app ID
/// stands in until an `AppDataChanged` callback fills it.
pub fn describe(session: &Session, app_id: u32, kind: &str, source: Source) -> GameEntry {
    GameEntry {
        name: session
            .app_name(app_id)
            .unwrap_or_else(|| format!("App {app_id}")),
        capsule_url: capsule_url(session, app_id),
        kind: kind.to_string(),
        source,
        has_schema: session.paths().schema_path(app_id).is_file(),
        app_id,
    }
}

/// Best available capsule art for a game.
///
/// Mirrors `GamePicker.GetGameImageUrl`: prefer the localized small capsule,
/// fall back to English, then to the legacy community `logo`.
pub fn capsule_url(session: &Session, app_id: u32) -> Option<String> {
    let language = session.current_game_language();

    if !language.is_empty() {
        if let Some(capsule) = session.app_data(app_id, &format!("small_capsule/{language}")) {
            return Some(store_asset_url(app_id, &capsule));
        }
    }

    if !language.eq_ignore_ascii_case("english") {
        if let Some(capsule) = session.app_data(app_id, "small_capsule/english") {
            return Some(store_asset_url(app_id, &capsule));
        }
    }

    session
        .app_data(app_id, "logo")
        .map(|logo| community_asset_url(app_id, &format!("{logo}.jpg")))
}

fn store_asset_url(app_id: u32, file: &str) -> String {
    format!(
        "https://shared.cloudflare.steamstatic.com/store_item_assets/steam/apps/{app_id}/{file}"
    )
}

/// Achievement icons and legacy logos live under the community CDN.
pub fn community_asset_url(app_id: u32, file: &str) -> String {
    format!("https://cdn.steamstatic.com/steamcommunity/public/images/apps/{app_id}/{file}")
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_the_games_document() {
        let xml = r#"<?xml version="1.0"?>
            <games>
              <game>480</game>
              <game type="demo">1234</game>
              <game type="junk">9999</game>
            </games>"#;

        let apps = parse_games_xml(xml);
        assert_eq!(apps.len(), 3);
        assert_eq!(apps[0].app_id, 480);
        // A missing type attribute means an ordinary game.
        assert_eq!(apps[0].kind, "normal");
        assert_eq!(apps[1].kind, "demo");
        assert_eq!(apps[2].app_id, 9999);
    }

    #[test]
    fn ignores_malformed_entries() {
        let xml = "<games><game>notanumber</game><game>7</game></games>";
        let apps = parse_games_xml(xml);
        assert_eq!(apps.len(), 1);
        assert_eq!(apps[0].app_id, 7);
    }

    #[test]
    fn reads_library_paths_from_text_vdf() {
        let vdf = r#"
"libraryfolders"
{
	"0"
	{
		"path"		"/home/someone/.local/share/Steam"
		"label"		""
	}
	"1"
	{
		"path"		"/mnt/games/SteamLibrary"
	}
}"#;
        let paths = quoted_values_for_key(vdf, "path");
        assert_eq!(
            paths,
            vec![
                "/home/someone/.local/share/Steam",
                "/mnt/games/SteamLibrary"
            ]
        );
        // The key match must not pick up "label".
        assert_eq!(quoted_values_for_key(vdf, "label"), vec![""]);
    }

    #[test]
    fn builds_cdn_urls() {
        assert_eq!(
            store_asset_url(620, "capsule_231x87.jpg"),
            "https://shared.cloudflare.steamstatic.com/store_item_assets/steam/apps/620/capsule_231x87.jpg"
        );
        assert_eq!(
            community_asset_url(620, "abc.jpg"),
            "https://cdn.steamstatic.com/steamcommunity/public/images/apps/620/abc.jpg"
        );
    }
}
