//! Draining Steam's callback queue.
//!
//! Steam does not push callbacks; a client polls `Steam_BGetCallback` and must
//! call `Steam_FreeLastCallback` after each one. The Windows original drives
//! this from a WinForms timer. Here the UI calls [`CallbackPump::poll`] once
//! per frame, which is a natural fit for an immediate-mode GUI.

use crate::client::Session;
use crate::ffi::{self, callback_id, AppDataChanged, UserStatsReceived};

/// A decoded callback.
#[derive(Debug, Clone, PartialEq)]
pub enum CallbackEvent {
    /// Stats for an app finished loading. `result` is an `EResult`;
    /// [`ffi::RESULT_OK`] (1) means success.
    UserStatsReceived {
        game_id: u64,
        result: i32,
        steam_id: u64,
    },
    /// A `StoreStats` call completed.
    UserStatsStored { game_id: u64, result: i32 },
    /// Steam finished fetching metadata for an app, so `GetAppData` will now
    /// return a name and logo where it previously returned nothing.
    AppDataChanged { app_id: u32, result: bool },
    /// Something we do not model. Carried through so callers can log it.
    Other { id: i32 },
}

/// Polls one session's callback queue.
pub struct CallbackPump<'a> {
    session: &'a Session,
}

impl<'a> CallbackPump<'a> {
    pub fn new(session: &'a Session) -> Self {
        Self { session }
    }

    /// Drain every queued callback.
    ///
    /// Each payload is copied out before the callback is freed, so nothing
    /// returned here points into Steam-owned memory.
    ///
    /// `limit` bounds the work done in one call so a flood cannot stall the
    /// UI thread indefinitely; leftovers are picked up on the next poll.
    pub fn poll_bounded(&self, limit: usize) -> Vec<CallbackEvent> {
        let library = self.session.library();
        let pipe = self.session.pipe();
        let mut events = Vec::new();

        for _ in 0..limit {
            let Some(msg) = library.next_callback(pipe) else {
                break;
            };

            // Copy the payload out while it is still alive. `msg.id` is read
            // through a local because the struct is packed and fields cannot
            // be borrowed directly.
            let id = msg.id;
            let event = match id {
                callback_id::USER_STATS_RECEIVED => {
                    // SAFETY: Steam guarantees the payload matches the id.
                    unsafe { ffi::read_payload::<UserStatsReceived>(&msg) }.map(|p| {
                        let (game_id, result, steam_id) = (p.game_id, p.result, p.steam_id_user);
                        CallbackEvent::UserStatsReceived {
                            game_id,
                            result,
                            steam_id,
                        }
                    })
                }
                callback_id::USER_STATS_STORED => {
                    // Same leading layout as UserStatsReceived: {u64, i32, ...}.
                    // SAFETY: as above.
                    unsafe { ffi::read_payload::<UserStatsReceived>(&msg) }.map(|p| {
                        let (game_id, result) = (p.game_id, p.result);
                        CallbackEvent::UserStatsStored { game_id, result }
                    })
                }
                callback_id::APP_DATA_CHANGED => {
                    // SAFETY: as above.
                    unsafe { ffi::read_payload::<AppDataChanged>(&msg) }.map(|p| {
                        let (app_id, result) = (p.app_id, p.result);
                        CallbackEvent::AppDataChanged {
                            app_id,
                            result: result != 0,
                        }
                    })
                }
                other => Some(CallbackEvent::Other { id: other }),
            };

            // Free before handling, matching the C# loop, so an early return
            // can never leak the callback slot.
            library.free_last_callback(pipe);

            if let Some(event) = event {
                events.push(event);
            }
        }

        events
    }

    /// Drain with a sensible default bound.
    pub fn poll(&self) -> Vec<CallbackEvent> {
        self.poll_bounded(256)
    }
}
