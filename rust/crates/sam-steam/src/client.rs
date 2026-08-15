//! Connecting to the running Steam client.
//!
//! [`Steam`] owns the loaded library and is created once per process.
//! [`Session`] is one connection (a pipe plus a global user) scoped to a
//! particular app, and is torn down and recreated when the user switches
//! games.

use std::ffi::{c_char, c_int, c_void, CString};
use std::sync::Arc;

use crate::error::{Error, Result};
use crate::ffi::{self, Interface, SteamLibrary};
use crate::interfaces::version;
use crate::interfaces::{steam_apps_001, steam_apps_008, steam_client, steam_user, steam_utils};
use crate::paths::SteamPaths;

/// A loaded `steamclient.so`, shared by every [`Session`].
///
/// `dlopen` is reference counted and Steam keeps per-process state, so the
/// library is opened once and kept for the lifetime of the program.
pub struct Steam {
    library: Arc<SteamLibrary>,
    paths: SteamPaths,
}

impl Steam {
    /// Locate and load Steam's client library.
    pub fn load() -> Result<Self> {
        let paths = SteamPaths::discover()?;
        Self::load_from(paths)
    }

    pub fn load_from(paths: SteamPaths) -> Result<Self> {
        let library = SteamLibrary::open(paths.client_library())?;
        Ok(Self {
            library: Arc::new(library),
            paths,
        })
    }

    pub fn paths(&self) -> &SteamPaths {
        &self.paths
    }

    /// Open a session.
    ///
    /// `app_id` sets the `SteamAppId` environment variable before the pipe is
    /// created, which is how the client decides which app this process is
    /// acting as. Pass `None` for a library-wide session, which is enough for
    /// browsing owned apps but not for reading or writing their stats.
    ///
    /// # Environment mutation
    ///
    /// Setting `SteamAppId` is process-global and not thread safe. Call this
    /// from the main thread and never concurrently with other environment
    /// access. A single-window UI naturally satisfies that.
    pub fn connect(&self, app_id: Option<u32>) -> Result<Session> {
        match app_id {
            Some(id) => std::env::set_var("SteamAppId", id.to_string()),
            // Removing it matters: a stale value from a previous session would
            // otherwise pin the new one to the wrong app.
            None => std::env::remove_var("SteamAppId"),
        }

        Session::open(Arc::clone(&self.library), self.paths.clone(), app_id)
    }
}

/// One connection to Steam, scoped to at most one app.
pub struct Session {
    library: Arc<SteamLibrary>,
    paths: SteamPaths,
    app_id: Option<u32>,
    pipe: c_int,
    user: c_int,
    steam_client: Interface,
    steam_user: Interface,
    steam_user_stats: Interface,
    steam_apps_001: Interface,
    steam_apps_008: Interface,
    steam_utils: Interface,
}

impl Session {
    fn open(library: Arc<SteamLibrary>, paths: SteamPaths, app_id: Option<u32>) -> Result<Self> {
        let steam_client = library.create_interface(version::STEAM_CLIENT)?;

        // Before calling anything, confirm the client vtable really is the
        // shape we expect. Every later call depends on this.
        steam_client.verify_slots(
            "SteamClient018",
            library.path(),
            &[
                (steam_client::CREATE_STEAM_PIPE, "CreateSteamPipe"),
                (steam_client::CONNECT_TO_GLOBAL_USER, "ConnectToGlobalUser"),
                (steam_client::GET_ISTEAM_USER_STATS, "GetISteamUserStats"),
                (steam_client::GET_ISTEAM_APPS, "GetISteamApps"),
            ],
        )?;

        // SAFETY: slots verified above; signatures follow the SysV convention
        // with an explicit leading `this`.
        let pipe = unsafe {
            let f: unsafe extern "C" fn(*mut c_void) -> c_int =
                steam_client.func(steam_client::CREATE_STEAM_PIPE);
            f(steam_client.as_ptr())
        };
        if pipe == 0 {
            return Err(Error::CreateSteamPipe);
        }

        // SAFETY: as above; `pipe` is a valid handle we just obtained.
        let user = unsafe {
            let f: unsafe extern "C" fn(*mut c_void, c_int) -> c_int =
                steam_client.func(steam_client::CONNECT_TO_GLOBAL_USER);
            f(steam_client.as_ptr(), pipe)
        };
        if user == 0 {
            // Release the pipe we opened before giving up.
            // SAFETY: `pipe` is valid and not yet released.
            unsafe {
                let f: unsafe extern "C" fn(*mut c_void, c_int) -> u8 =
                    steam_client.func(steam_client::RELEASE_STEAM_PIPE);
                f(steam_client.as_ptr(), pipe);
            }
            return Err(Error::ConnectToGlobalUser);
        }

        let mut session = Self {
            library,
            paths,
            app_id,
            pipe,
            user,
            steam_client,
            // Placeholders, replaced immediately below. Using the client
            // pointer keeps them non-null until then.
            steam_user: steam_client,
            steam_user_stats: steam_client,
            steam_apps_001: steam_client,
            steam_apps_008: steam_client,
            steam_utils: steam_client,
        };

        session.steam_utils = session.get_utils(version::STEAM_UTILS)?;
        session.steam_user = session.get_user(version::STEAM_USER)?;
        session.steam_user_stats = session.get_user_stats(version::STEAM_USER_STATS)?;
        session.steam_apps_001 = session.get_apps(version::STEAM_APPS_001)?;
        session.steam_apps_008 = session.get_apps(version::STEAM_APPS_008)?;

        // If we asked to act as a specific app, make sure Steam agrees. A
        // mismatch means the environment variable did not take effect and
        // every stat call would silently target the wrong game.
        if let Some(requested) = app_id {
            let actual = session.current_app_id();
            if actual != requested {
                return Err(Error::AppIdMismatch { requested, actual });
            }
        }

        Ok(session)
    }

    fn get_utils(&self, version: &'static str) -> Result<Interface> {
        let c_version = CString::new(version).expect("ASCII literal");
        // SAFETY: GetISteamUtils takes (this, pipe, version).
        let ptr = unsafe {
            let f: unsafe extern "C" fn(*mut c_void, c_int, *const c_char) -> *mut c_void =
                self.steam_client.func(steam_client::GET_ISTEAM_UTILS);
            f(self.steam_client.as_ptr(), self.pipe, c_version.as_ptr())
        };
        Interface::new(ptr).ok_or(Error::GetInterface("ISteamUtils"))
    }

    fn get_user(&self, version: &'static str) -> Result<Interface> {
        let c_version = CString::new(version).expect("ASCII literal");
        // SAFETY: GetISteamUser takes (this, user, pipe, version).
        let ptr = unsafe {
            let f: unsafe extern "C" fn(*mut c_void, c_int, c_int, *const c_char) -> *mut c_void =
                self.steam_client.func(steam_client::GET_ISTEAM_USER);
            f(
                self.steam_client.as_ptr(),
                self.user,
                self.pipe,
                c_version.as_ptr(),
            )
        };
        Interface::new(ptr).ok_or(Error::GetInterface("ISteamUser"))
    }

    fn get_user_stats(&self, version: &'static str) -> Result<Interface> {
        let c_version = CString::new(version).expect("ASCII literal");
        // SAFETY: GetISteamUserStats takes (this, user, pipe, version).
        let ptr = unsafe {
            let f: unsafe extern "C" fn(*mut c_void, c_int, c_int, *const c_char) -> *mut c_void =
                self.steam_client.func(steam_client::GET_ISTEAM_USER_STATS);
            f(
                self.steam_client.as_ptr(),
                self.user,
                self.pipe,
                c_version.as_ptr(),
            )
        };
        Interface::new(ptr).ok_or(Error::GetInterface("ISteamUserStats"))
    }

    fn get_apps(&self, version: &'static str) -> Result<Interface> {
        let c_version = CString::new(version).expect("ASCII literal");
        // SAFETY: GetISteamApps takes (this, user, pipe, version). The C#
        // delegate omits `this`, which only works under thiscall; on SysV the
        // receiver is a real argument and must be passed.
        let ptr = unsafe {
            let f: unsafe extern "C" fn(*mut c_void, c_int, c_int, *const c_char) -> *mut c_void =
                self.steam_client.func(steam_client::GET_ISTEAM_APPS);
            f(
                self.steam_client.as_ptr(),
                self.user,
                self.pipe,
                c_version.as_ptr(),
            )
        };
        Interface::new(ptr).ok_or(Error::GetInterface("ISteamApps"))
    }

    pub fn paths(&self) -> &SteamPaths {
        &self.paths
    }

    pub fn library(&self) -> &SteamLibrary {
        &self.library
    }

    pub fn app_id(&self) -> Option<u32> {
        self.app_id
    }

    pub(crate) fn pipe(&self) -> c_int {
        self.pipe
    }

    pub(crate) fn user_stats_interface(&self) -> Interface {
        self.steam_user_stats
    }

    /// `ISteamUtils::GetAppID`, the app Steam believes this process is.
    pub fn current_app_id(&self) -> u32 {
        // SAFETY: GetAppID takes only `this` and returns a uint32.
        unsafe {
            let f: unsafe extern "C" fn(*mut c_void) -> u32 =
                self.steam_utils.func(steam_utils::GET_APP_ID);
            f(self.steam_utils.as_ptr())
        }
    }

    /// The signed-in user's 64-bit Steam ID.
    ///
    /// `CSteamID` wraps a single `uint64` and is trivially copyable, so SysV
    /// returns it in RAX exactly like a plain `u64`.
    pub fn steam_id(&self) -> u64 {
        // SAFETY: GetSteamID takes only `this`.
        unsafe {
            let f: unsafe extern "C" fn(*mut c_void) -> u64 =
                self.steam_user.func(steam_user::GET_STEAM_ID);
            f(self.steam_user.as_ptr())
        }
    }

    /// `ISteamApps::IsSubscribedApp` — whether the signed-in account owns `app_id`.
    pub fn owns_app(&self, app_id: u32) -> bool {
        // SAFETY: IsSubscribedApp takes (this, AppId_t) and returns bool.
        unsafe {
            let f: unsafe extern "C" fn(*mut c_void, u32) -> u8 =
                self.steam_apps_008.func(steam_apps_008::IS_SUBSCRIBED_APP);
            f(self.steam_apps_008.as_ptr(), app_id) != 0
        }
    }

    /// `ISteamApps::BIsAppInstalled`.
    pub fn is_app_installed(&self, app_id: u32) -> bool {
        // SAFETY: takes (this, AppId_t), returns bool.
        unsafe {
            let f: unsafe extern "C" fn(*mut c_void, u32) -> u8 =
                self.steam_apps_008.func(steam_apps_008::IS_APP_INSTALLED);
            f(self.steam_apps_008.as_ptr(), app_id) != 0
        }
    }

    /// The client's current game language, e.g. `"english"`.
    pub fn current_game_language(&self) -> String {
        // SAFETY: takes only `this`, returns a static `const char*`.
        unsafe {
            let f: unsafe extern "C" fn(*mut c_void) -> *const c_char = self
                .steam_apps_008
                .func(steam_apps_008::GET_CURRENT_GAME_LANGUAGE);
            ffi::string_from_ptr(f(self.steam_apps_008.as_ptr())).unwrap_or_default()
        }
    }

    /// `ISteamApps001::GetAppData(appId, key)`.
    ///
    /// This is the app metadata cache the picker relies on: `"name"`,
    /// `"logo"`, `"small_capsule/<language>"` and so on. Returns `None` when
    /// Steam has no value cached, which is common for apps never launched.
    ///
    /// Unlike most string-returning methods on these interfaces, this one does
    /// *not* hand back a `const char*`. Its real signature is
    /// `int GetAppData(AppId_t, const char *key, char *value, int valueLen)`:
    /// it fills a caller-provided buffer and returns the number of bytes
    /// written, or 0 when there is no value. Treating that `int` as a pointer
    /// dereferences a small integer and segfaults.
    pub fn app_data(&self, app_id: u32, key: &str) -> Option<String> {
        let c_key = CString::new(key).ok()?;
        // 1024 matches the buffer the C# allocates. App data values are short
        // (names, capsule file names); anything longer is truncated rather
        // than overflowing, because we also pass the capacity.
        let mut buffer = vec![0u8; 1024];

        // SAFETY: `buffer` is exclusively borrowed and its true capacity is
        // passed as `valueLen`, so Steam cannot write past the end.
        let written = unsafe {
            let f: unsafe extern "C" fn(
                *mut c_void,
                u32,
                *const c_char,
                *mut c_char,
                c_int,
            ) -> c_int = self.steam_apps_001.func(steam_apps_001::GET_APP_DATA);
            f(
                self.steam_apps_001.as_ptr(),
                app_id,
                c_key.as_ptr(),
                buffer.as_mut_ptr() as *mut c_char,
                buffer.len() as c_int,
            )
        };

        if written <= 0 {
            return None;
        }

        // Steam NUL-terminates, but bound by the reported length as well so a
        // misbehaving value cannot run past what was actually written.
        let limit = (written as usize).min(buffer.len());
        let end = buffer[..limit]
            .iter()
            .position(|&b| b == 0)
            .unwrap_or(limit);
        let value = String::from_utf8_lossy(&buffer[..end]).into_owned();
        (!value.is_empty()).then_some(value)
    }

    /// The display name of an app, if Steam has it cached.
    pub fn app_name(&self, app_id: u32) -> Option<String> {
        self.app_data(app_id, "name")
    }
}

impl Drop for Session {
    fn drop(&mut self) {
        // SAFETY: both handles are still valid here, and this runs once.
        // Releasing in the reverse order of acquisition mirrors the C#.
        unsafe {
            if self.user != 0 {
                let f: unsafe extern "C" fn(*mut c_void, c_int, c_int) =
                    self.steam_client.func(steam_client::RELEASE_USER);
                f(self.steam_client.as_ptr(), self.pipe, self.user);
                self.user = 0;
            }
            if self.pipe != 0 {
                let f: unsafe extern "C" fn(*mut c_void, c_int) -> u8 =
                    self.steam_client.func(steam_client::RELEASE_STEAM_PIPE);
                f(self.steam_client.as_ptr(), self.pipe);
                self.pipe = 0;
            }
        }
    }
}
