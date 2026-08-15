//! The achievement and statistic schema for one app.
//!
//! Steam's client interface exposes stat and achievement *values* only. Every
//! piece of metadata — display names, descriptions, icon file names, min/max
//! bounds and the permission bits that mark a stat protected — comes from the
//! binary KeyValues blob Steam caches at
//! `<steam>/appcache/stats/UserGameStatsSchema_<appid>.bin`.
//!
//! Ported from `Manager.LoadUserGameStatsSchema` in the C#.

use std::path::Path;

use sam_vdf::KeyValue;

use crate::error::Result;

/// `UserStatType` from the Steam client.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum UserStatType {
    Invalid = 0,
    Integer = 1,
    Float = 2,
    AverageRate = 3,
    Achievements = 4,
    GroupAchievements = 5,
}

impl UserStatType {
    /// Newer schemas store the type as a string; `Int` is an alias for
    /// `Integer`, which is why the C# enum carries both spellings.
    fn from_name(name: &str) -> Option<Self> {
        match name.to_ascii_lowercase().as_str() {
            "invalid" => Some(Self::Invalid),
            "int" | "integer" => Some(Self::Integer),
            "float" => Some(Self::Float),
            "avgrate" | "averagerate" => Some(Self::AverageRate),
            "achievements" => Some(Self::Achievements),
            "groupachievements" => Some(Self::GroupAchievements),
            _ => None,
        }
    }

    fn from_raw(raw: i32) -> Self {
        match raw {
            1 => Self::Integer,
            2 => Self::Float,
            3 => Self::AverageRate,
            4 => Self::Achievements,
            5 => Self::GroupAchievements,
            _ => Self::Invalid,
        }
    }
}

/// Permission bits. Anything with bit 1 set is server-controlled and cannot be
/// edited; the UI must refuse rather than let Steam reject the write later.
pub const PERMISSION_PROTECTED_MASK: i32 = 3;

/// Whether a definition's permission bits mark it protected.
pub fn is_protected(permission: i32) -> bool {
    permission & PERMISSION_PROTECTED_MASK != 0
}

#[derive(Debug, Clone, PartialEq)]
pub struct AchievementDefinition {
    pub id: String,
    pub name: String,
    pub description: String,
    pub icon_normal: String,
    pub icon_locked: String,
    pub hidden: bool,
    pub permission: i32,
}

impl AchievementDefinition {
    pub fn is_protected(&self) -> bool {
        is_protected(self.permission)
    }

    /// The icon file to show for a given unlock state, falling back to the
    /// unlocked icon when a game ships no greyed-out variant.
    pub fn icon_for(&self, unlocked: bool) -> &str {
        if unlocked || self.icon_locked.is_empty() {
            &self.icon_normal
        } else {
            &self.icon_locked
        }
    }
}

#[derive(Debug, Clone, PartialEq)]
pub enum StatBounds {
    Integer {
        min: i32,
        max: i32,
        max_change: i32,
        default: i32,
    },
    Float {
        min: f32,
        max: f32,
        max_change: f32,
        default: f32,
    },
}

#[derive(Debug, Clone, PartialEq)]
pub struct StatDefinition {
    pub id: String,
    pub display_name: String,
    pub bounds: StatBounds,
    pub increment_only: bool,
    pub permission: i32,
}

impl StatDefinition {
    pub fn is_integer(&self) -> bool {
        matches!(self.bounds, StatBounds::Integer { .. })
    }

    pub fn is_protected(&self) -> bool {
        is_protected(self.permission)
    }
}

#[derive(Debug, Clone, PartialEq, Default)]
pub struct GameSchema {
    pub app_id: u32,
    pub achievements: Vec<AchievementDefinition>,
    pub stats: Vec<StatDefinition>,
}

impl GameSchema {
    /// Read and parse the cached schema for `app_id`.
    ///
    /// `language` selects localized display strings, with the usual fallback
    /// chain: requested language, then English, then the bare node value.
    pub fn load(path: &Path, app_id: u32, language: &str) -> Result<Self> {
        let data = std::fs::read(path)?;
        let kv = sam_vdf::parse(&data)?;
        Ok(Self::from_keyvalues(&kv, app_id, language))
    }

    /// Build from an already-parsed document. Split out so it can be tested
    /// against synthetic input without touching the filesystem.
    pub fn from_keyvalues(root: &KeyValue, app_id: u32, language: &str) -> Self {
        let mut schema = Self {
            app_id,
            ..Default::default()
        };

        // The document's single child is keyed by the app ID as a string.
        // Fall back to the first child so a mismatched file still parses.
        let app_key = app_id.to_string();
        let app_node = {
            let by_id = root.get(&app_key);
            if by_id.is_valid() {
                by_id
            } else if let Some(first) = root.children.first() {
                first
            } else {
                return schema;
            }
        };

        for stat in &app_node.get("stats").children {
            match resolve_stat_type(stat) {
                UserStatType::Integer => {
                    if let Some(def) = parse_integer_stat(stat, language) {
                        schema.stats.push(def);
                    }
                }
                UserStatType::Float | UserStatType::AverageRate => {
                    if let Some(def) = parse_float_stat(stat, language) {
                        schema.stats.push(def);
                    }
                }
                UserStatType::Achievements | UserStatType::GroupAchievements => {
                    collect_achievements(stat, language, &mut schema.achievements);
                }
                UserStatType::Invalid => {}
            }
        }

        schema
    }

    /// Integer stat names, used to probe the vtable overload order.
    pub fn integer_stat_names(&self) -> Vec<String> {
        self.stats
            .iter()
            .filter(|s| s.is_integer())
            .map(|s| s.id.clone())
            .collect()
    }

    /// Float stat names, the secondary probe for the overload order.
    pub fn float_stat_names(&self) -> Vec<String> {
        self.stats
            .iter()
            .filter(|s| !s.is_integer())
            .map(|s| s.id.clone())
            .collect()
    }

    pub fn achievement(&self, id: &str) -> Option<&AchievementDefinition> {
        self.achievements.iter().find(|a| a.id == id)
    }
}

/// Both schema dialects: a `type` string in newer files, a numeric `type_int`
/// (or numeric `type`) in older ones. Trying the string first and falling back
/// mirrors the C#, which had to keep working across a Steam-side format change.
fn resolve_stat_type(stat: &KeyValue) -> UserStatType {
    let type_node = stat.get("type");

    if let Some(name) = type_node.as_str() {
        if let Some(parsed) = UserStatType::from_name(name) {
            if parsed != UserStatType::Invalid {
                return parsed;
            }
        }
    }

    let type_int_node = stat.get("type_int");
    let raw = if type_int_node.is_valid() {
        type_int_node.as_i32_or(0)
    } else {
        type_node.as_i32_or(0)
    };
    UserStatType::from_raw(raw)
}

/// Localized string lookup: requested language, then English, then the node's
/// own value, then the supplied default.
fn localized(node: &KeyValue, language: &str, default: &str) -> String {
    let preferred = node.get(language).as_string_or("");
    if !preferred.is_empty() {
        return preferred;
    }
    if !language.eq_ignore_ascii_case("english") {
        let english = node.get("english").as_string_or("");
        if !english.is_empty() {
            return english;
        }
    }
    let bare = node.as_string_or("");
    if !bare.is_empty() {
        return bare;
    }
    default.to_string()
}

fn parse_integer_stat(stat: &KeyValue, language: &str) -> Option<StatDefinition> {
    let id = stat.get("name").as_string_or("");
    if id.is_empty() {
        return None;
    }
    let display_name = localized(stat.get("display").get("name"), language, &id);
    Some(StatDefinition {
        bounds: StatBounds::Integer {
            min: stat.get("min").as_i32_or(i32::MIN),
            max: stat.get("max").as_i32_or(i32::MAX),
            max_change: stat.get("maxchange").as_i32_or(0),
            default: stat.get("default").as_i32_or(0),
        },
        increment_only: stat.get("incrementonly").as_bool_or(false),
        permission: stat.get("permission").as_i32_or(0),
        id,
        display_name,
    })
}

fn parse_float_stat(stat: &KeyValue, language: &str) -> Option<StatDefinition> {
    let id = stat.get("name").as_string_or("");
    if id.is_empty() {
        return None;
    }
    let display_name = localized(stat.get("display").get("name"), language, &id);
    Some(StatDefinition {
        bounds: StatBounds::Float {
            min: stat.get("min").as_f32_or(f32::MIN),
            max: stat.get("max").as_f32_or(f32::MAX),
            max_change: stat.get("maxchange").as_f32_or(0.0),
            default: stat.get("default").as_f32_or(0.0),
        },
        increment_only: stat.get("incrementonly").as_bool_or(false),
        permission: stat.get("permission").as_i32_or(0),
        id,
        display_name,
    })
}

/// Achievement entries hang off repeated `bits` containers, so a
/// single-result lookup would silently drop most of them.
fn collect_achievements(stat: &KeyValue, language: &str, out: &mut Vec<AchievementDefinition>) {
    for bits in stat.get_all("bits") {
        for bit in &bits.children {
            let id = bit.get("name").as_string_or("");
            if id.is_empty() {
                continue;
            }
            let display = bit.get("display");
            let name = localized(display.get("name"), language, &id);
            let description = localized(display.get("desc"), language, "");
            out.push(AchievementDefinition {
                icon_normal: display.get("icon").as_string_or(""),
                icon_locked: display.get("icon_gray").as_string_or(""),
                hidden: display.get("hidden").as_bool_or(false),
                permission: bit.get("permission").as_i32_or(0),
                id,
                name,
                description,
            });
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Serialises a minimal schema document in the binary KeyValues format.
    struct Builder(Vec<u8>);

    impl Builder {
        fn new() -> Self {
            Self(Vec::new())
        }
        fn open(mut self, name: &str) -> Self {
            self.0.push(0);
            self.0.extend_from_slice(name.as_bytes());
            self.0.push(0);
            self
        }
        fn string(mut self, name: &str, value: &str) -> Self {
            self.0.push(1);
            self.0.extend_from_slice(name.as_bytes());
            self.0.push(0);
            self.0.extend_from_slice(value.as_bytes());
            self.0.push(0);
            self
        }
        fn int(mut self, name: &str, value: i32) -> Self {
            self.0.push(2);
            self.0.extend_from_slice(name.as_bytes());
            self.0.push(0);
            self.0.extend_from_slice(&value.to_le_bytes());
            self
        }
        fn close(mut self) -> Self {
            self.0.push(8);
            self
        }
        fn build(self) -> Vec<u8> {
            self.0
        }
    }

    fn parse(data: &[u8], app_id: u32, language: &str) -> GameSchema {
        let kv = sam_vdf::parse(data).expect("fixture should parse");
        GameSchema::from_keyvalues(&kv, app_id, language)
    }

    #[test]
    fn reads_new_style_string_types() {
        let data = Builder::new()
            .open("480")
            .open("stats")
            .open("1")
            .string("type", "int")
            .string("name", "kills")
            .int("permission", 0)
            .open("display")
            .open("name")
            .string("english", "Kills")
            .close()
            .close()
            .close()
            .close()
            .close()
            .close()
            .build();

        let schema = parse(&data, 480, "english");
        assert_eq!(schema.stats.len(), 1);
        assert_eq!(schema.stats[0].id, "kills");
        assert_eq!(schema.stats[0].display_name, "Kills");
        assert!(schema.stats[0].is_integer());
    }

    #[test]
    fn reads_legacy_type_int_dialect() {
        // Older caches carry a numeric type_int instead of a type string.
        let data = Builder::new()
            .open("480")
            .open("stats")
            .open("1")
            .int("type_int", 1)
            .string("name", "legacy_stat")
            .close()
            .close()
            .close()
            .close()
            .build();

        let schema = parse(&data, 480, "english");
        assert_eq!(schema.stats.len(), 1);
        assert!(schema.stats[0].is_integer());
    }

    #[test]
    fn collects_achievements_from_repeated_bits_blocks() {
        let mut b = Builder::new().open("480").open("stats").open("1");
        b = b.string("type", "achievements");
        for i in 0..2 {
            b = b
                .open("bits")
                .open("0")
                .string("name", &format!("ACH_{i}"))
                .open("display")
                .open("name")
                .string("english", &format!("Achievement {i}"))
                .close()
                .string("icon", "on.jpg")
                .string("icon_gray", "off.jpg")
                .close()
                .close()
                .close();
        }
        // Closes, innermost first: "1", "stats", "480", then the root.
        let data = b.close().close().close().close().build();

        let schema = parse(&data, 480, "english");
        assert_eq!(schema.achievements.len(), 2);
        assert_eq!(schema.achievements[0].id, "ACH_0");
        assert_eq!(schema.achievements[1].name, "Achievement 1");
        assert_eq!(schema.achievements[0].icon_for(true), "on.jpg");
        assert_eq!(schema.achievements[0].icon_for(false), "off.jpg");
    }

    #[test]
    fn falls_back_to_english_then_to_id() {
        let data = Builder::new()
            .open("480")
            .open("stats")
            .open("1")
            .string("type", "int")
            .string("name", "only_english")
            .open("display")
            .open("name")
            .string("english", "English Name")
            .close()
            .close()
            .close()
            .open("2")
            .string("type", "int")
            .string("name", "no_display_at_all")
            .close()
            .close()
            .close()
            .close()
            .build();

        // German is absent, so the English string is used.
        let schema = parse(&data, 480, "german");
        assert_eq!(schema.stats[0].display_name, "English Name");
        // With no display block at all, the stat id stands in.
        assert_eq!(schema.stats[1].display_name, "no_display_at_all");
    }

    #[test]
    fn missing_icon_gray_falls_back_to_the_unlocked_icon() {
        let def = AchievementDefinition {
            id: "A".into(),
            name: "A".into(),
            description: String::new(),
            icon_normal: "on.jpg".into(),
            icon_locked: String::new(),
            hidden: false,
            permission: 0,
        };
        assert_eq!(def.icon_for(false), "on.jpg");
    }

    #[test]
    fn permission_bits_mark_protected_entries() {
        assert!(!is_protected(0));
        assert!(is_protected(1));
        assert!(is_protected(2));
        assert!(is_protected(3));
        // Bits outside the mask do not imply protection on their own.
        assert!(!is_protected(4));
    }
}
