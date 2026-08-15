//! Native Linux bindings to Steam's private `steamclient.so` interfaces.
//!
//! This is the Rust port of `SAM.API` from the C# Steam Achievement Manager.
//! It exists so the tool can run as an ordinary native Linux process talking
//! to the native Steam client, rather than under Wine, where the Windows build
//! cannot see a Steam client living outside the prefix.
//!
//! # Why the private interfaces
//!
//! The public Steamworks SDK (`libsteam_api.so`) can only act on behalf of one
//! app that launched through Steam, and offers no way to enumerate a library.
//! SAM needs `GetAppData` and `IsSubscribedApp` for arbitrary app IDs, so it
//! goes through `steamclient.so` directly, as the Windows version always has.
//!
//! # Shape of the API
//!
//! ```no_run
//! use sam_steam::{Steam, UserStats, GameSchema};
//!
//! // Load the library once per process.
//! let steam = Steam::load()?;
//!
//! // A session with no app ID is enough to browse the library.
//! let browsing = steam.connect(None)?;
//! let owned = browsing.owns_app(480);
//! drop(browsing);
//!
//! // Editing an app's stats needs a session scoped to it.
//! let session = steam.connect(Some(480))?;
//! let schema = GameSchema::load(&session.paths().schema_path(480), 480, "english")?;
//! let mut stats = UserStats::new(&session)?;
//! stats.request_user_stats(session.steam_id());
//! // ... pump callbacks until UserStatsReceived arrives, then:
//! stats.resolve_overload_order(&schema.integer_stat_names(), &schema.float_stat_names());
//! # Ok::<(), sam_steam::Error>(())
//! ```
//!
//! # One process, one app
//!
//! Steam reads `SteamAppId` when `steamclient.so` initialises, not when a pipe
//! is opened. Setting it afterwards has no effect, and a process that has
//! bound to one app cannot rebind to another — [`Steam::connect`] will return
//! [`Error::AppIdMismatch`]. Editing a second game therefore requires a second
//! process, which is why the Windows original ships a separate `SAM.Game.exe`.
//!
//! # Safety posture
//!
//! Every call crosses into C++ through a vtable, so a layout mismatch is the
//! central risk. Two mitigations run before anything is invoked:
//!
//! 1. [`ffi::Interface::verify_slots`] uses `dladdr` to prove each slot we
//!    intend to call resolves to a symbol inside `steamclient.so`.
//! 2. [`user_stats::UserStats::resolve_overload_order`] determines the
//!    `int32`/`float` overload ordering empirically, using reads only, rather
//!    than trusting a compiled-in assumption.

pub mod callbacks;
pub mod client;
pub mod error;
pub mod ffi;
pub mod interfaces;
pub mod paths;
pub mod schema;
pub mod user_stats;

pub use callbacks::{CallbackEvent, CallbackPump};
pub use client::{Session, Steam};
pub use error::{Error, Result};
pub use paths::SteamPaths;
pub use schema::{
    AchievementDefinition, GameSchema, StatBounds, StatDefinition, UserStatType,
    PERMISSION_PROTECTED_MASK,
};
pub use user_stats::{OrderConfidence, OrderResolution, OverloadOrder, UserStats};
