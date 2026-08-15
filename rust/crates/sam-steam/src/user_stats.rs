//! `ISteamUserStats013`: reading and writing achievements and statistics.

use std::ffi::{c_char, c_void, CString};

use crate::client::Session;
use crate::error::Result;
use crate::ffi::{self, Interface};
use crate::interfaces::steam_user_stats as slot;

/// Which of an overloaded pair comes first in the vtable.
///
/// `GetStat`, `SetStat` and `GetUserStat` are each declared twice in
/// `ISteamUserStats`, once for `int32` and once for `float`. The Itanium C++
/// ABI lays overloads out in declaration order (int first); MSVC reverses
/// them within an overload group, which is why the C# interface definitions
/// list the float form first.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum OverloadOrder {
    /// Itanium / GCC: `int32` occupies the lower slot. Expected on Linux.
    IntThenFloat,
    /// MSVC: `float` occupies the lower slot. Expected on Windows.
    FloatThenInt,
}

impl OverloadOrder {
    fn int_offset(self) -> usize {
        match self {
            OverloadOrder::IntThenFloat => 0,
            OverloadOrder::FloatThenInt => 1,
        }
    }

    fn float_offset(self) -> usize {
        1 - self.int_offset()
    }
}

/// How confident we are in the overload order currently in use.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum OrderConfidence {
    /// Proven against the running library by probing a stat of known type.
    Confirmed,
    /// No integer stat was available to probe with, so the ABI default is in
    /// use. Reads are still safe; writes should be treated with more care.
    Assumed(&'static str),
}

/// Result of [`UserStats::resolve_overload_order`].
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct OrderResolution {
    pub order: OverloadOrder,
    pub confidence: OrderConfidence,
    /// The stat used to prove the order, when one was found.
    pub probe_stat: Option<String>,
}

/// Achievement and statistic access for one [`Session`].
pub struct UserStats<'a> {
    session: &'a Session,
    interface: Interface,
    order: OverloadOrder,
    confidence: OrderConfidence,
}

impl<'a> UserStats<'a> {
    /// Wrap a session's stats interface, verifying the unambiguous vtable
    /// slots before anything is called through them.
    pub fn new(session: &'a Session) -> Result<Self> {
        let interface = session.user_stats_interface();

        interface.verify_slots(
            "ISteamUserStats013",
            session.library().path(),
            &[
                (slot::GET_STAT_BASE, "GetStat[0]"),
                (slot::GET_STAT_BASE + 1, "GetStat[1]"),
                (slot::SET_STAT_BASE, "SetStat[0]"),
                (slot::SET_STAT_BASE + 1, "SetStat[1]"),
                (slot::GET_ACHIEVEMENT, "GetAchievement"),
                (slot::SET_ACHIEVEMENT, "SetAchievement"),
                (slot::CLEAR_ACHIEVEMENT, "ClearAchievement"),
                (
                    slot::GET_ACHIEVEMENT_AND_UNLOCK_TIME,
                    "GetAchievementAndUnlockTime",
                ),
                (slot::STORE_STATS, "StoreStats"),
                (slot::GET_NUM_ACHIEVEMENTS, "GetNumAchievements"),
                (slot::REQUEST_USER_STATS, "RequestUserStats"),
                (slot::RESET_ALL_STATS, "ResetAllStats"),
            ],
        )?;

        Ok(Self {
            session,
            interface,
            // Start from the ABI-derived expectation; callers should follow up
            // with `resolve_overload_order` once a schema is available.
            order: OverloadOrder::IntThenFloat,
            confidence: OrderConfidence::Assumed("not yet probed"),
        })
    }

    /// Rebuild a handle for a session already validated by [`Self::new`],
    /// reusing a previously resolved overload order.
    ///
    /// Verification and probing are one-time costs; this exists so a UI can
    /// hold a `Session` and mint a short-lived `UserStats` per interaction
    /// without paying for `dladdr` lookups on every frame.
    pub fn reuse(session: &'a Session, order: OverloadOrder, confidence: OrderConfidence) -> Self {
        Self {
            interface: session.user_stats_interface(),
            session,
            order,
            confidence,
        }
    }

    pub fn order(&self) -> OverloadOrder {
        self.order
    }

    pub fn confidence(&self) -> &OrderConfidence {
        &self.confidence
    }

    /// Determine the real overload order by probing stats of known type.
    ///
    /// Steam type-checks `GetStat`: it rejects a stat whose declared type does
    /// not match the overload, so asking for an integer stat through the
    /// `float` overload returns false.
    ///
    /// The discriminator is the *slot*, not the pointer type passed in. A
    /// callee cannot see what kind of pointer the caller supplied — it writes
    /// four bytes and answers based on the stat — so varying the out-pointer
    /// type proves nothing, and only varying which slot is invoked does.
    /// Both slots of the group are therefore called with a stat whose type the
    /// schema already gives us: for an `int` stat, the slot returning true is
    /// the `int32` overload.
    ///
    /// This reads only. Nothing is written, so an inconclusive result is
    /// harmless. Call it once the app's stats have arrived, since a stat that
    /// has never been written may be accepted by both slots.
    pub fn resolve_overload_order(
        &mut self,
        integer_stat_names: &[String],
        float_stat_names: &[String],
    ) -> OrderResolution {
        // Integer stats first: they are far more common than float ones.
        for (names, kind_is_int) in [(integer_stat_names, true), (float_stat_names, false)] {
            for name in names {
                let Ok(c_name) = CString::new(name.as_str()) else {
                    continue;
                };

                let first = self.probe_slot(&c_name, slot::GET_STAT_BASE);
                let second = self.probe_slot(&c_name, slot::GET_STAT_BASE + 1);

                // Exactly one slot should accept a stat of known type.
                // Both or neither means Steam is not type-checking this stat
                // (seen when a stat has never been written), so move on.
                let accepting = match (first, second) {
                    (true, false) => 0usize,
                    (false, true) => 1usize,
                    _ => continue,
                };

                // If an integer stat is accepted by slot 0, int comes first.
                // For a float stat the mapping inverts.
                let order = match (accepting, kind_is_int) {
                    (0, true) | (1, false) => OverloadOrder::IntThenFloat,
                    _ => OverloadOrder::FloatThenInt,
                };

                self.order = order;
                self.confidence = OrderConfidence::Confirmed;
                return OrderResolution {
                    order,
                    confidence: OrderConfidence::Confirmed,
                    probe_stat: Some(name.clone()),
                };
            }
        }

        let reason = if integer_stat_names.is_empty() && float_stat_names.is_empty() {
            "this app declares no numeric stats"
        } else {
            "no stat produced an unambiguous result"
        };
        self.order = OverloadOrder::IntThenFloat;
        self.confidence = OrderConfidence::Assumed(reason);
        OrderResolution {
            order: self.order,
            confidence: OrderConfidence::Assumed(reason),
            probe_stat: None,
        }
    }

    /// Invoke one `GetStat` slot purely for its boolean result.
    ///
    /// The out-pointer is an 8-byte scratch value, which is large enough for
    /// whichever of `int32*` or `float*` this slot actually expects.
    fn probe_slot(&self, name: &CString, index: usize) -> bool {
        let mut scratch: u64 = 0;
        // SAFETY: slot verified in `new`. Both overloads take
        // `(this, const char*, T*)` where `T` is 4 bytes, so an 8-byte
        // exclusively-borrowed local is always a valid destination.
        unsafe {
            let f: unsafe extern "C" fn(*mut c_void, *const c_char, *mut c_void) -> u8 =
                self.interface.func(index);
            f(
                self.interface.as_ptr(),
                name.as_ptr(),
                &mut scratch as *mut u64 as *mut c_void,
            ) != 0
        }
    }

    /// Ask Steam to load this user's stats for the current app.
    ///
    /// Returns the API call handle. The `UserStatsReceived` callback (id 1101)
    /// fires when the data has arrived; see [`crate::callbacks`].
    pub fn request_user_stats(&self, steam_id: u64) -> u64 {
        // SAFETY: takes (this, CSteamID). CSteamID is a trivially copyable
        // 8-byte wrapper, so it is passed in a register like a u64.
        unsafe {
            let f: unsafe extern "C" fn(*mut c_void, u64) -> u64 =
                self.interface.func(slot::REQUEST_USER_STATS);
            f(self.interface.as_ptr(), steam_id)
        }
    }

    pub fn num_achievements(&self) -> u32 {
        // SAFETY: takes only `this`.
        unsafe {
            let f: unsafe extern "C" fn(*mut c_void) -> u32 =
                self.interface.func(slot::GET_NUM_ACHIEVEMENTS);
            f(self.interface.as_ptr())
        }
    }

    pub fn achievement_name(&self, index: u32) -> Option<String> {
        // SAFETY: takes (this, uint32), returns a `const char*` owned by Steam.
        unsafe {
            let f: unsafe extern "C" fn(*mut c_void, u32) -> *const c_char =
                self.interface.func(slot::GET_ACHIEVEMENT_NAME);
            ffi::string_from_ptr(f(self.interface.as_ptr(), index))
        }
    }

    /// Whether `name` is unlocked, and when. `None` if Steam does not know the
    /// achievement, which usually means stats have not loaded yet.
    pub fn achievement_and_unlock_time(&self, name: &str) -> Option<(bool, u32)> {
        let c_name = CString::new(name).ok()?;
        let mut achieved: u8 = 0;
        let mut unlock_time: u32 = 0;
        // SAFETY: signature is
        // `bool GetAchievementAndUnlockTime(this, const char*, bool*, uint32*)`.
        // `achieved` is read as u8 because C++ `bool` may hold any byte.
        let ok = unsafe {
            let f: unsafe extern "C" fn(*mut c_void, *const c_char, *mut u8, *mut u32) -> u8 =
                self.interface.func(slot::GET_ACHIEVEMENT_AND_UNLOCK_TIME);
            f(
                self.interface.as_ptr(),
                c_name.as_ptr(),
                &mut achieved,
                &mut unlock_time,
            ) != 0
        };
        ok.then_some((achieved != 0, unlock_time))
    }

    pub fn achievement(&self, name: &str) -> Option<bool> {
        let c_name = CString::new(name).ok()?;
        let mut achieved: u8 = 0;
        // SAFETY: `bool GetAchievement(this, const char*, bool*)`.
        let ok = unsafe {
            let f: unsafe extern "C" fn(*mut c_void, *const c_char, *mut u8) -> u8 =
                self.interface.func(slot::GET_ACHIEVEMENT);
            f(self.interface.as_ptr(), c_name.as_ptr(), &mut achieved) != 0
        };
        ok.then_some(achieved != 0)
    }

    /// Lock or unlock an achievement. Local until [`Self::store_stats`].
    pub fn set_achievement(&self, name: &str, unlocked: bool) -> bool {
        let Ok(c_name) = CString::new(name) else {
            return false;
        };
        let index = if unlocked {
            slot::SET_ACHIEVEMENT
        } else {
            slot::CLEAR_ACHIEVEMENT
        };
        // SAFETY: both Set and Clear take (this, const char*) and return bool.
        unsafe {
            let f: unsafe extern "C" fn(*mut c_void, *const c_char) -> u8 =
                self.interface.func(index);
            f(self.interface.as_ptr(), c_name.as_ptr()) != 0
        }
    }

    pub fn achievement_display_attribute(&self, name: &str, key: &str) -> Option<String> {
        let c_name = CString::new(name).ok()?;
        let c_key = CString::new(key).ok()?;
        // SAFETY: takes (this, const char*, const char*), returns Steam-owned
        // string valid until the next call; copied immediately.
        unsafe {
            let f: unsafe extern "C" fn(
                *mut c_void,
                *const c_char,
                *const c_char,
            ) -> *const c_char = self.interface.func(slot::GET_ACHIEVEMENT_DISPLAY_ATTRIBUTE);
            ffi::string_from_ptr(f(self.interface.as_ptr(), c_name.as_ptr(), c_key.as_ptr()))
        }
    }

    pub fn stat_i32(&self, name: &str) -> Option<i32> {
        let c_name = CString::new(name).ok()?;
        let mut out: i32 = 0;
        let index = slot::GET_STAT_BASE + self.order.int_offset();
        // SAFETY: `bool GetStat(this, const char*, int32*)` at the resolved slot.
        let ok = unsafe {
            let f: unsafe extern "C" fn(*mut c_void, *const c_char, *mut i32) -> u8 =
                self.interface.func(index);
            f(self.interface.as_ptr(), c_name.as_ptr(), &mut out) != 0
        };
        ok.then_some(out)
    }

    pub fn stat_f32(&self, name: &str) -> Option<f32> {
        let c_name = CString::new(name).ok()?;
        let mut out: f32 = 0.0;
        let index = slot::GET_STAT_BASE + self.order.float_offset();
        // SAFETY: `bool GetStat(this, const char*, float*)` at the resolved slot.
        let ok = unsafe {
            let f: unsafe extern "C" fn(*mut c_void, *const c_char, *mut f32) -> u8 =
                self.interface.func(index);
            f(self.interface.as_ptr(), c_name.as_ptr(), &mut out) != 0
        };
        ok.then_some(out)
    }

    pub fn set_stat_i32(&self, name: &str, value: i32) -> bool {
        let Ok(c_name) = CString::new(name) else {
            return false;
        };
        let index = slot::SET_STAT_BASE + self.order.int_offset();
        // SAFETY: `bool SetStat(this, const char*, int32)` at the resolved slot.
        unsafe {
            let f: unsafe extern "C" fn(*mut c_void, *const c_char, i32) -> u8 =
                self.interface.func(index);
            f(self.interface.as_ptr(), c_name.as_ptr(), value) != 0
        }
    }

    pub fn set_stat_f32(&self, name: &str, value: f32) -> bool {
        let Ok(c_name) = CString::new(name) else {
            return false;
        };
        let index = slot::SET_STAT_BASE + self.order.float_offset();
        // SAFETY: `bool SetStat(this, const char*, float)` at the resolved slot.
        unsafe {
            let f: unsafe extern "C" fn(*mut c_void, *const c_char, f32) -> u8 =
                self.interface.func(index);
            f(self.interface.as_ptr(), c_name.as_ptr(), value) != 0
        }
    }

    /// Commit every pending achievement and stat change to Steam.
    pub fn store_stats(&self) -> bool {
        // SAFETY: takes only `this`.
        unsafe {
            let f: unsafe extern "C" fn(*mut c_void) -> u8 = self.interface.func(slot::STORE_STATS);
            f(self.interface.as_ptr()) != 0
        }
    }

    /// Reset every stat for this app, optionally wiping achievements too.
    pub fn reset_all_stats(&self, achievements_too: bool) -> bool {
        // SAFETY: `bool ResetAllStats(this, bool)`.
        unsafe {
            let f: unsafe extern "C" fn(*mut c_void, u8) -> u8 =
                self.interface.func(slot::RESET_ALL_STATS);
            f(self.interface.as_ptr(), u8::from(achievements_too)) != 0
        }
    }

    pub fn session(&self) -> &Session {
        self.session
    }
}
