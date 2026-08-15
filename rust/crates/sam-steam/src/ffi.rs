//! Raw interop with `steamclient.so`.
//!
//! # How this works
//!
//! Steam's private client API hands out pointers to C++ objects. There are no C
//! entry points for the methods, so every call goes through the object's
//! vtable by index. The layout is the platform C++ ABI:
//!
//! * The object's first word is a pointer to its vtable.
//! * Under the Itanium C++ ABI (GCC/Clang on Linux), that pointer already
//!   points at the first virtual function, past the offset-to-top and RTTI
//!   slots, exactly as it does under MSVC. So slot *n* is `vtable[n]`.
//! * Virtual functions appear in declaration order.
//!
//! # Where Linux differs from the Windows original
//!
//! * **Calling convention.** Windows x86 uses `thiscall`, which passes `this`
//!   in ECX. SysV x86-64 has no such thing: `this` is simply the first
//!   argument, in RDI. Every signature here therefore takes an explicit
//!   `this: *mut c_void` first parameter and uses plain `extern "C"`.
//!   (The C# has a latent bug here: `NativeGetISteamApps` omits `self`
//!   entirely and only works because `thiscall` keeps `this` out of the stack
//!   arguments. Reproducing that omission on Linux would shift every argument.)
//!
//! * **Overload ordering.** MSVC emits overloads within one overload group in
//!   *reverse* declaration order; the Itanium ABI uses plain declaration
//!   order. `ISteamUserStats` overloads `GetStat`, `SetStat` and `GetUserStat`
//!   on `int32` versus `float`, so those pairs are swapped relative to the C#
//!   interface definitions. See [`crate::user_stats`], which detects the real
//!   order at run time instead of trusting this reasoning.
//!
//! * **Callback packing.** The Steamworks SDK defines
//!   `VALVE_CALLBACK_PACK_SMALL` (`#pragma pack(4)`) on Linux and macOS, and
//!   `PACK_LARGE` (pack 8) on Windows, specifically so that 64-bit callback
//!   structures keep the same layout as 32-bit ones. Hence `packed(4)` below
//!   rather than natural alignment.
//!
//! * **Booleans.** C++ `bool` is one byte and Steam is not guaranteed to
//!   return exactly 0 or 1. Rust's `bool` has exactly two valid bit patterns,
//!   so materialising a foreign byte as `bool` would be undefined behaviour.
//!   Every FFI boundary here uses `u8` and normalises with `!= 0`.

use std::ffi::{c_char, c_int, c_void, CStr, CString};
use std::path::{Path, PathBuf};

use crate::error::{Error, Result};

// ---------------------------------------------------------------------------
// Callback payloads
// ---------------------------------------------------------------------------

/// `CallbackMsg_t`. Under pack(4): `{i32 @0, i32 @4, ptr @8, i32 @16}`, 20 bytes.
#[repr(C, packed(4))]
#[derive(Clone, Copy)]
pub struct CallbackMsg {
    pub user: c_int,
    pub id: c_int,
    pub param: *mut u8,
    pub param_size: c_int,
}

impl Default for CallbackMsg {
    fn default() -> Self {
        Self {
            user: 0,
            id: 0,
            param: std::ptr::null_mut(),
            param_size: 0,
        }
    }
}

/// `UserStatsReceived_t`, callback id 1101.
/// Under pack(4): `{u64 @0, i32 @8, u64 @12}`, 20 bytes.
#[repr(C, packed(4))]
#[derive(Clone, Copy, Default)]
pub struct UserStatsReceived {
    pub game_id: u64,
    pub result: c_int,
    pub steam_id_user: u64,
}

/// `AppDataChanged_t`, callback id 1001.
#[repr(C, packed(4))]
#[derive(Clone, Copy, Default)]
pub struct AppDataChanged {
    pub app_id: u32,
    pub result: u8,
}

/// `EResult::k_EResultOK`.
pub const RESULT_OK: c_int = 1;

pub mod callback_id {
    pub const USER_STATS_RECEIVED: i32 = 1101;
    pub const USER_STATS_STORED: i32 = 1102;
    pub const APP_DATA_CHANGED: i32 = 1001;
}

// ---------------------------------------------------------------------------
// dladdr, used to sanity check vtable slots
// ---------------------------------------------------------------------------

#[repr(C)]
struct DlInfo {
    dli_fname: *const c_char,
    dli_fbase: *mut c_void,
    dli_sname: *const c_char,
    dli_saddr: *mut c_void,
}

extern "C" {
    fn dladdr(addr: *const c_void, info: *mut DlInfo) -> c_int;
}

/// The shared object containing `addr`, via `dladdr`.
///
/// Used to prove a vtable slot points into `steamclient.so` before we call it.
/// A short vtable or a layout change usually yields a null slot, a pointer
/// into some unrelated library, or an address `dladdr` cannot resolve at all —
/// all of which this catches before the call happens.
fn owning_object(addr: *const c_void) -> Option<PathBuf> {
    if addr.is_null() {
        return None;
    }
    // SAFETY: `info` is a well-formed out-parameter; `dladdr` only writes it.
    unsafe {
        let mut info = DlInfo {
            dli_fname: std::ptr::null(),
            dli_fbase: std::ptr::null_mut(),
            dli_sname: std::ptr::null(),
            dli_saddr: std::ptr::null_mut(),
        };
        if dladdr(addr, &mut info) == 0 || info.dli_fname.is_null() {
            return None;
        }
        let name = CStr::from_ptr(info.dli_fname)
            .to_string_lossy()
            .into_owned();
        Some(PathBuf::from(name))
    }
}

// ---------------------------------------------------------------------------
// Interface handles
// ---------------------------------------------------------------------------

/// A pointer to a Steam C++ interface object.
///
/// `Copy` because Steam owns the storage; these are borrowed views with no
/// drop semantics. Lifetime is tied to the owning [`crate::Client`], which
/// keeps the library loaded and the pipe open.
#[derive(Clone, Copy)]
pub struct Interface {
    ptr: *mut c_void,
}

impl Interface {
    pub(crate) fn new(ptr: *mut c_void) -> Option<Self> {
        (!ptr.is_null()).then_some(Self { ptr })
    }

    pub(crate) fn as_ptr(self) -> *mut c_void {
        self.ptr
    }

    /// The raw address stored in vtable slot `index`.
    ///
    /// # Safety
    /// The caller must know that the object really has a vtable with at least
    /// `index + 1` entries. [`Self::verify_slots`] is the intended way to
    /// establish that.
    unsafe fn raw_slot(self, index: usize) -> *const c_void {
        let vtable = *(self.ptr as *const *const *const c_void);
        *vtable.add(index)
    }

    /// Reinterpret vtable slot `index` as a function pointer.
    ///
    /// # Safety
    /// `F` must be an `extern "C"` function pointer type whose signature
    /// matches the C++ method at that slot, including the leading `this`.
    /// Getting this wrong is undefined behaviour, which is why every slot
    /// index in this crate is a named constant checked by `verify_slots`.
    pub(crate) unsafe fn func<F: Copy>(self, index: usize) -> F {
        const {
            assert!(
                std::mem::size_of::<F>() == std::mem::size_of::<*const c_void>(),
                "F must be a plain (non-fat) function pointer"
            );
        }
        let addr = self.raw_slot(index);
        std::mem::transmute_copy(&addr)
    }

    /// Check that the given slots all resolve into `library`.
    ///
    /// This is cheap insurance against a Steam update reshuffling an
    /// interface: rather than calling into whatever happens to sit at the
    /// index, we notice up front and refuse.
    pub(crate) fn verify_slots(
        self,
        interface: &'static str,
        library: &Path,
        slots: &[(usize, &'static str)],
    ) -> Result<()> {
        for &(index, name) in slots {
            // SAFETY: reading the slot address does not call it. If the vtable
            // were short enough for this to read out of bounds we would be in
            // trouble already, but in practice these objects live in Steam's
            // heap with the vtable in .rodata, and a wrong read yields an
            // address `dladdr` rejects below.
            let addr = unsafe { self.raw_slot(index) };
            match owning_object(addr) {
                Some(owner) if owner == library => {}
                Some(owner) => {
                    return Err(Error::VtableSanityCheckFailed(format!(
                        "{interface} slot {index} ({name}) resolves into {} \
                         rather than {}",
                        owner.display(),
                        library.display()
                    )))
                }
                None => {
                    return Err(Error::VtableSanityCheckFailed(format!(
                        "{interface} slot {index} ({name}) is null or unresolvable"
                    )))
                }
            }
        }
        Ok(())
    }
}

// ---------------------------------------------------------------------------
// The library itself
// ---------------------------------------------------------------------------

type CreateInterfaceFn = unsafe extern "C" fn(*const c_char, *mut c_int) -> *mut c_void;
type BGetCallbackFn = unsafe extern "C" fn(c_int, *mut CallbackMsg, *mut c_int) -> u8;
type FreeLastCallbackFn = unsafe extern "C" fn(c_int) -> u8;

/// An open handle to `steamclient.so` plus its three private entry points.
pub struct SteamLibrary {
    path: PathBuf,
    create_interface: CreateInterfaceFn,
    get_callback: BGetCallbackFn,
    free_last_callback: FreeLastCallbackFn,
    /// Kept alive so the code we hold pointers into stays mapped. Must be
    /// declared last so it drops after everything above.
    _library: libloading::Library,
}

impl SteamLibrary {
    pub fn open(path: &Path) -> Result<Self> {
        // SAFETY: dlopen of a Valve-shipped library. Its initialisers run, as
        // they must for the client to work at all.
        let library =
            unsafe { libloading::Library::new(path) }.map_err(|e| Error::LoadLibrary {
                path: path.to_path_buf(),
                source: e.to_string(),
            })?;

        // SAFETY: each symbol's declared type matches the documented private
        // ABI; the pointers stay valid as long as `library` is alive, which is
        // guaranteed by storing it in the same struct.
        unsafe {
            let create_interface = *library
                .get::<CreateInterfaceFn>(b"CreateInterface\0")
                .map_err(|_| Error::MissingExport("CreateInterface"))?;
            let get_callback = *library
                .get::<BGetCallbackFn>(b"Steam_BGetCallback\0")
                .map_err(|_| Error::MissingExport("Steam_BGetCallback"))?;
            let free_last_callback = *library
                .get::<FreeLastCallbackFn>(b"Steam_FreeLastCallback\0")
                .map_err(|_| Error::MissingExport("Steam_FreeLastCallback"))?;

            Ok(Self {
                path: path.to_path_buf(),
                create_interface,
                get_callback,
                free_last_callback,
                _library: library,
            })
        }
    }

    pub fn path(&self) -> &Path {
        &self.path
    }

    /// `CreateInterface(version, nullptr)`.
    pub fn create_interface(&self, version: &'static str) -> Result<Interface> {
        let c_version = CString::new(version).expect("interface versions are ASCII literals");
        // SAFETY: `c_version` outlives the call; Steam copies what it needs.
        let ptr = unsafe { (self.create_interface)(c_version.as_ptr(), std::ptr::null_mut()) };
        Interface::new(ptr).ok_or(Error::CreateInterface(version))
    }

    /// Pop one queued callback for `pipe`, if any.
    ///
    /// The returned payload pointer is owned by Steam and stays valid only
    /// until [`Self::free_last_callback`] is called for the same pipe, so
    /// callers must copy anything they need out first.
    pub fn next_callback(&self, pipe: c_int) -> Option<CallbackMsg> {
        let mut msg = CallbackMsg::default();
        let mut call: c_int = 0;
        // SAFETY: both out-parameters are valid, exclusively borrowed locals.
        // We pass three arguments to match the three-argument form the C#
        // uses; if this build of Steam takes only two, the extra register is
        // simply ignored under SysV.
        let got = unsafe { (self.get_callback)(pipe, &mut msg, &mut call) };
        (got != 0).then_some(msg)
    }

    /// Release the callback most recently returned by [`Self::next_callback`].
    pub fn free_last_callback(&self, pipe: c_int) {
        // SAFETY: plain scalar argument; safe to call even with nothing queued.
        unsafe {
            (self.free_last_callback)(pipe);
        }
    }
}

/// Copy a callback payload out of Steam-owned memory.
///
/// # Safety
/// `msg.param` must point to at least `size_of::<T>()` readable bytes, which
/// Steam guarantees when `msg.id` is the id matching `T`.
pub unsafe fn read_payload<T: Copy + Default>(msg: &CallbackMsg) -> Option<T> {
    let param = msg.param;
    let size = msg.param_size;
    if param.is_null() || (size as usize) < std::mem::size_of::<T>() {
        return None;
    }
    Some(std::ptr::read_unaligned(param as *const T))
}

/// Borrow a Steam-returned `const char*` as an owned `String`.
///
/// # Safety
/// `ptr` must be null or a valid NUL-terminated string.
pub unsafe fn string_from_ptr(ptr: *const c_char) -> Option<String> {
    if ptr.is_null() {
        return None;
    }
    Some(CStr::from_ptr(ptr).to_string_lossy().into_owned())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn callback_structs_match_the_linux_pack4_abi() {
        // If these ever change, callbacks silently decode to garbage.
        assert_eq!(std::mem::size_of::<CallbackMsg>(), 20);
        assert_eq!(std::mem::size_of::<UserStatsReceived>(), 20);
    }

    #[test]
    fn dladdr_resolves_a_known_local_symbol() {
        // Proves the sanity-check mechanism itself works in this environment.
        let addr = dladdr_probe as *const c_void;
        assert!(owning_object(addr).is_some());
    }

    extern "C" fn dladdr_probe() {}
}
