//! Vtable slot indices for the Steam interfaces this tool uses.
//!
//! These are **Linux / Itanium C++ ABI** indices: virtual functions in plain
//! declaration order. They are ported from the C# `SAM.API/Interfaces/*.cs`
//! structures, which describe the *MSVC* layout.
//!
//! For interfaces without overloaded methods the two layouts agree, and the
//! indices below are a direct transcription. `ISteamUserStats` is the
//! exception: MSVC orders an overload group in reverse declaration order, so
//! its `int32`/`float` pairs are swapped relative to the C#. Those pairs are
//! deliberately *not* hardcoded here — see [`crate::user_stats::OverloadOrder`],
//! which resolves them against the running library.

/// `SteamClient018`. No overloads, so identical to the MSVC layout.
pub mod steam_client {
    pub const CREATE_STEAM_PIPE: usize = 0;
    pub const RELEASE_STEAM_PIPE: usize = 1;
    pub const CONNECT_TO_GLOBAL_USER: usize = 2;
    pub const CREATE_LOCAL_USER: usize = 3;
    pub const RELEASE_USER: usize = 4;
    pub const GET_ISTEAM_USER: usize = 5;
    pub const GET_ISTEAM_UTILS: usize = 9;
    pub const GET_ISTEAM_USER_STATS: usize = 13;
    pub const GET_ISTEAM_APPS: usize = 15;
}

/// `STEAMAPPS_INTERFACE_VERSION001`. A single method.
pub mod steam_apps_001 {
    pub const GET_APP_DATA: usize = 0;
}

/// `STEAMAPPS_INTERFACE_VERSION008`. No overloads.
pub mod steam_apps_008 {
    pub const IS_SUBSCRIBED: usize = 0;
    pub const GET_CURRENT_GAME_LANGUAGE: usize = 4;
    pub const IS_SUBSCRIBED_APP: usize = 6;
    pub const IS_APP_INSTALLED: usize = 20;
}

/// `SteamUser012`. No overloads.
pub mod steam_user {
    pub const GET_STEAM_ID: usize = 2;
}

/// `SteamUtils005`. No overloads.
pub mod steam_utils {
    pub const GET_APP_ID: usize = 9;
}

/// `STEAMUSERSTATS_INTERFACE_VERSION013`.
///
/// Slots listed here are the unambiguous, non-overloaded ones. The overloaded
/// `GetStat` (0/1), `SetStat` (2/3) and `GetUserStat` (16/17) pairs are
/// resolved at run time.
pub mod steam_user_stats {
    /// First slot of the `GetStat` overload group.
    pub const GET_STAT_BASE: usize = 0;
    /// First slot of the `SetStat` overload group.
    pub const SET_STAT_BASE: usize = 2;

    pub const UPDATE_AVG_RATE_STAT: usize = 4;
    pub const GET_ACHIEVEMENT: usize = 5;
    pub const SET_ACHIEVEMENT: usize = 6;
    pub const CLEAR_ACHIEVEMENT: usize = 7;
    pub const GET_ACHIEVEMENT_AND_UNLOCK_TIME: usize = 8;
    pub const STORE_STATS: usize = 9;
    pub const GET_ACHIEVEMENT_ICON: usize = 10;
    pub const GET_ACHIEVEMENT_DISPLAY_ATTRIBUTE: usize = 11;
    pub const INDICATE_ACHIEVEMENT_PROGRESS: usize = 12;
    pub const GET_NUM_ACHIEVEMENTS: usize = 13;
    pub const GET_ACHIEVEMENT_NAME: usize = 14;
    pub const REQUEST_USER_STATS: usize = 15;
    pub const RESET_ALL_STATS: usize = 20;
}

/// Interface version strings passed to `CreateInterface` and the
/// `GetISteam*` getters.
pub mod version {
    pub const STEAM_CLIENT: &str = "SteamClient018";
    pub const STEAM_USER: &str = "SteamUser012";
    pub const STEAM_USER_STATS: &str = "STEAMUSERSTATS_INTERFACE_VERSION013";
    pub const STEAM_APPS_001: &str = "STEAMAPPS_INTERFACE_VERSION001";
    pub const STEAM_APPS_008: &str = "STEAMAPPS_INTERFACE_VERSION008";
    pub const STEAM_UTILS: &str = "SteamUtils005";
}
