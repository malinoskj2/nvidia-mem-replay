//! Live ShadowPlay settings through NVIDIA's own API library (`nvspapi64.dll`).
//!
//! The ShadowPlay engine (hosted by `nvcontainer.exe`) reads `TempFilePath` from the
//! registry only when it starts. Later registry edits are ignored until the engine
//! restarts, and toggling Instant Replay does not re-read them. The NVIDIA overlay
//! changes the setting at run time through this library instead: the value travels over
//! NVIDIA's message bus to the running engine, which applies it immediately and persists
//! it, including the derived Highlights path, to the registry. This module uses the same
//! entry point.
//!
//! The interface is undocumented. Its layout follows the open-source Experienceless
//! client (MIT) and was verified against NVIDIA App 11.0.9 (`nvspapi64.dll` 11.0.9.251),
//! whose `CaptureCore.log` reports every structure-version and client-id mismatch.
//! Structure versions carry the structure size in their low 16 bits.

use std::{
    ffi::c_void, io, iter::once, os::windows::ffi::OsStrExt, path::PathBuf, ptr, sync::Mutex,
};
use thiserror::Error;
use winreg::{
    RegKey,
    enums::{HKEY_LOCAL_MACHINE, KEY_READ, KEY_WOW64_64KEY},
};

/// Instant Replay's temporary-files location, as named by the overlay.
pub(crate) const TEMPORARY_PATH: &str = "TempFilePath";

const LIBRARY: &str = "nvspapi64.dll";
const OVERLAY_KEY: &str = r"SOFTWARE\NVIDIA Corporation\Global\Overlay";
const OVERLAY_INSTALL_PATH: &str = "InstallPath";
const PROGRAM_FILES: &str = "ProgramFiles";
const INSTALL_DIRECTORIES: [&str; 2] = [
    r"NVIDIA Corporation\NVIDIA App\ShadowPlay",
    r"NVIDIA Corporation\ShadowPlay",
];
const CREATE_INTERFACE: &[u8] = b"CreateShadowPlayApiInterface\0";
const LOAD_WITH_ALTERED_SEARCH_PATH: u32 = 0x0000_0008;

/// Low 16 bits: `size_of::<CreateParams>()`.
const CREATE_PARAMS_VERSION: u32 = 0x0001_0018;
/// The first-generation `IShadowPlayApi` table of twenty methods; slots 6, 7 and 15 are used here.
const INTERFACE_VERSION: u32 = 0x0001_0008;
/// Each client id joins the message bus under its own module name, and a second registration
/// under a name already in use is dropped (its calls then time out). The library accepts 3
/// through 11; 5 is the overlay itself. The application uses 6 (`ShadowPlayApi_TestingTool`)
/// and its supervisor process 3 (`ShadowPlayApi_Installer`), so both can be connected at once.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum Role {
    Application,
    Supervisor,
}

impl Role {
    const fn client(self) -> u32 {
        match self {
            Self::Application => 6,
            Self::Supervisor => 3,
        }
    }
}

static ROLE: Mutex<Role> = Mutex::new(Role::Application);
/// Low 16 bits: `size_of::<PropertyArgs>()`.
const PROPERTY_ARGS_VERSION: u32 = 0x0001_0060;
const PROPERTY_NAME_CAPACITY: usize = 64;
const SET_PROPERTY_SLOT: usize = 6;
const GET_PROPERTY_SLOT: usize = 7;
const GET_SESSION_PARAM_SLOT: usize = 15;
const VT_BSTR: u16 = 8;
/// Low 16 bits: `size_of::<SessionParamArgs>()`.
const SESSION_PARAM_ARGS_VERSION: u32 = 0x0001_0020;
/// Engine-wide parameters are read without a capture session of our own.
const GLOBAL_SESSION: u64 = 0xFFFF_FFFF;
/// Parameter 12 reports whether Instant Replay is capturing right now.
const INSTANT_REPLAY_STATE_COMMAND: u32 = 12;
const INSTANT_REPLAY_STATE_HEADER: u16 = 0x1c;
const PARAM_VALUE_VERSION: u16 = 1;
const PARAM_VALUE_SIZE: u32 = 12;
const INSTANT_REPLAY_STATE: &str = "Instant Replay state";

#[derive(Debug, Error)]
pub(crate) enum ApiError {
    #[error("nvspapi64.dll was not found; install or repair the NVIDIA App")]
    NotFound,
    #[error("load {}: {source}", path.display())]
    Load {
        path: PathBuf,
        #[source]
        source: io::Error,
    },
    #[error("nvspapi64.dll does not export CreateShadowPlayApiInterface")]
    Export,
    #[error("CreateShadowPlayApiInterface failed with HRESULT {0:#010x}")]
    Create(i32),
    #[error(
        "ShadowPlay did not accept {name} (HRESULT {result:#010x}); make sure the NVIDIA overlay is running"
    )]
    Call { name: String, result: i32 },
    #[error("ShadowPlay returned no text for {0}")]
    NoText(String),
    #[error("ShadowPlay property names are ASCII and shorter than 64 bytes")]
    Name,
    #[error("out of memory while preparing a ShadowPlay call")]
    Memory,
    #[error("ShadowPlay API state is unavailable")]
    Lock,
}

impl ApiError {
    /// Whether the engine itself could not be reached: the library did not load, no interface
    /// was created, or the engine answered a call with a failure (its message bus times out while
    /// `nvcontainer.exe` is not running). A missing library or export is an installation
    /// problem, a missing value an answer, and the remaining variants are internal errors.
    pub(crate) const fn is_connection_failure(&self) -> bool {
        matches!(
            self,
            Self::Load { .. } | Self::Create(_) | Self::Call { .. }
        )
    }
}

#[link(name = "kernel32")]
unsafe extern "system" {
    fn LoadLibraryExW(file_name: *const u16, file: *mut c_void, flags: u32) -> *mut c_void;
    fn GetProcAddress(module: *mut c_void, name: *const u8) -> Option<unsafe extern "system" fn()>;
    fn FreeLibrary(module: *mut c_void) -> i32;
}

#[link(name = "oleaut32")]
unsafe extern "system" {
    fn SysAllocString(text: *const u16) -> *mut u16;
    fn SysFreeString(text: *mut u16);
    fn SysStringLen(text: *const u16) -> u32;
    fn VariantClear(variant: *mut Variant) -> i32;
}

type CreateInterface = unsafe extern "system" fn(params: *mut CreateParams) -> i32;
type Method = unsafe extern "system" fn(this: *mut c_void, args: *mut c_void) -> i32;

#[repr(C)]
struct CreateParams {
    version: u32,
    interface_version: u32,
    client: u32,
    interface: *mut *mut c_void,
}

/// Enough of a COM `VARIANT` to carry a `BSTR`.
#[repr(C)]
struct Variant {
    kind: u16,
    reserved: [u16; 3],
    data: [*mut c_void; 2],
}

#[repr(C)]
struct PropertyArgs {
    version: u32,
    name: [u8; PROPERTY_NAME_CAPACITY],
    reserved: u32,
    value: Variant,
}

#[repr(C)]
struct SessionParamArgs {
    version: u32,
    reserved: u32,
    session: u64,
    command: u32,
    size: u32,
    data: *mut ParamValue,
}

#[repr(C)]
struct ParamValue {
    header: u16,
    version: u16,
    value: u32,
    result: u32,
}

const _: () = assert!(size_of::<CreateParams>() == 0x18);
const _: () = assert!(size_of::<Variant>() == 24);
const _: () = assert!(size_of::<PropertyArgs>() == 0x60);
const _: () = assert!(size_of::<SessionParamArgs>() == 0x20);
const _: () = assert!(size_of::<ParamValue>() == PARAM_VALUE_SIZE as usize);

impl Variant {
    const EMPTY: Self = Self {
        kind: 0,
        reserved: [0; 3],
        data: [ptr::null_mut(); 2],
    };

    fn text(&self) -> Option<String> {
        if self.kind != VT_BSTR || self.data[0].is_null() {
            return None;
        }

        let text = self.data[0].cast::<u16>();
        // SAFETY: a VT_BSTR variant holds a valid BSTR; SysStringLen reads its length prefix.
        let units = unsafe { std::slice::from_raw_parts(text, SysStringLen(text) as usize) };
        Some(String::from_utf16_lossy(units))
    }
}

impl PropertyArgs {
    fn new(name: &str) -> Result<Self, ApiError> {
        (!name.is_empty() && name.is_ascii() && name.len() < PROPERTY_NAME_CAPACITY)
            .ok_or(ApiError::Name)?;

        let mut args = Self {
            version: PROPERTY_ARGS_VERSION,
            name: [0; PROPERTY_NAME_CAPACITY],
            reserved: 0,
            value: Variant::EMPTY,
        };
        args.name[..name.len()].copy_from_slice(name.as_bytes());
        Ok(args)
    }
}

/// One connection to the running `ShadowPlay` engine, kept for the life of the process.
struct Api {
    interface: *mut c_void,
    vtable: *const Method,
}

// SAFETY: the overlay drives this interface from several threads; calls here are serialized.
unsafe impl Send for Api {}

static SHARED: Mutex<Option<Api>> = Mutex::new(None);

/// The current value of a text property as the running engine reports it.
pub(crate) fn text(name: &str) -> Result<String, ApiError> {
    with_api(|api| api.text(name))
}

/// Set a text property; the engine applies it immediately and persists it to the registry.
pub(crate) fn set_text(name: &str, value: &str) -> Result<(), ApiError> {
    with_api(|api| api.set_text(name, value))
}

/// Whether Instant Replay is capturing right now, which is when its temporary files are open.
pub(crate) fn instant_replay_running() -> Result<bool, ApiError> {
    with_api(Api::instant_replay_running)
}

/// Choose the bus identity before the first call; the supervisor must differ from the GUI.
pub(crate) fn set_role(role: Role) {
    if let Ok(mut current) = ROLE.lock() {
        *current = role;
    }
}

fn with_api<T>(call: impl FnOnce(&Api) -> Result<T, ApiError>) -> Result<T, ApiError> {
    let mut shared = SHARED.lock().map_err(|_| ApiError::Lock)?;
    if shared.is_none() {
        *shared = Some(Api::create()?);
    }
    let api = shared.as_ref().ok_or(ApiError::Lock)?;
    call(api)
}

impl Api {
    fn create() -> Result<Self, ApiError> {
        let path = library_path()?;
        let file_name: Vec<u16> = path.as_os_str().encode_wide().chain(once(0)).collect();
        // SAFETY: `file_name` is a NUL-terminated UTF-16 string that outlives the call.
        let module = unsafe {
            LoadLibraryExW(
                file_name.as_ptr(),
                ptr::null_mut(),
                LOAD_WITH_ALTERED_SEARCH_PATH,
            )
        };
        if module.is_null() {
            return Err(ApiError::Load {
                path,
                source: io::Error::last_os_error(),
            });
        }

        // A connection is kept for the life of the process, so the library stays loaded only
        // once it has yielded an interface; a failed attempt releases it for the next retry.
        Self::connect(module).inspect_err(|_| unload(module))
    }

    fn connect(module: *mut c_void) -> Result<Self, ApiError> {
        // SAFETY: `module` is a loaded library and the export name is NUL-terminated.
        let Some(export) = (unsafe { GetProcAddress(module, CREATE_INTERFACE.as_ptr()) }) else {
            return Err(ApiError::Export);
        };
        // SAFETY: the export is the library's documented factory function.
        let create =
            unsafe { std::mem::transmute::<unsafe extern "system" fn(), CreateInterface>(export) };

        let role = ROLE.lock().map(|role| *role).map_err(|_| ApiError::Lock)?;
        let mut interface: *mut c_void = ptr::null_mut();
        let mut params = CreateParams {
            version: CREATE_PARAMS_VERSION,
            interface_version: INTERFACE_VERSION,
            client: role.client(),
            interface: &raw mut interface,
        };
        // SAFETY: `params` matches the version it declares; both pointers outlive the call.
        let result = unsafe { create(&raw mut params) };
        if result != 0 || interface.is_null() {
            return Err(ApiError::Create(result));
        }

        // SAFETY: a C++ interface pointer starts with its virtual table pointer.
        let vtable = unsafe { *interface.cast::<*const Method>() };
        if vtable.is_null() {
            return Err(ApiError::Create(result));
        }

        Ok(Self { interface, vtable })
    }

    fn method(&self, slot: usize) -> Method {
        // SAFETY: the library populates the twenty methods of the first-generation table.
        unsafe { *self.vtable.add(slot) }
    }

    fn text(&self, name: &str) -> Result<String, ApiError> {
        let mut args = PropertyArgs::new(name)?;
        // SAFETY: `args` matches PROPERTY_ARGS_VERSION; the library fills its VARIANT.
        let result =
            unsafe { (self.method(GET_PROPERTY_SLOT))(self.interface, (&raw mut args).cast()) };
        let value = if result == 0 { args.value.text() } else { None };
        // SAFETY: the VARIANT started empty and is owned by `args`.
        unsafe { VariantClear(&raw mut args.value) };

        if result != 0 {
            return Err(ApiError::Call {
                name: name.to_owned(),
                result,
            });
        }
        value.ok_or_else(|| ApiError::NoText(name.to_owned()))
    }

    fn set_text(&self, name: &str, value: &str) -> Result<(), ApiError> {
        let mut args = PropertyArgs::new(name)?;
        let units: Vec<u16> = value.encode_utf16().chain(once(0)).collect();
        // SAFETY: `units` is a NUL-terminated UTF-16 string.
        let text = unsafe { SysAllocString(units.as_ptr()) };
        if text.is_null() {
            return Err(ApiError::Memory);
        }
        args.value.kind = VT_BSTR;
        args.value.data[0] = text.cast();

        // SAFETY: `args` matches PROPERTY_ARGS_VERSION; the BSTR stays allocated during the call.
        let result =
            unsafe { (self.method(SET_PROPERTY_SLOT))(self.interface, (&raw mut args).cast()) };
        // SAFETY: `text` came from SysAllocString and the library copied the value.
        unsafe { SysFreeString(text) };

        if result == 0 {
            Ok(())
        } else {
            Err(ApiError::Call {
                name: name.to_owned(),
                result,
            })
        }
    }

    fn instant_replay_running(&self) -> Result<bool, ApiError> {
        let mut value = ParamValue {
            header: INSTANT_REPLAY_STATE_HEADER,
            version: PARAM_VALUE_VERSION,
            value: 0,
            result: 0,
        };
        let mut args = SessionParamArgs {
            version: SESSION_PARAM_ARGS_VERSION,
            reserved: 0,
            session: GLOBAL_SESSION,
            command: INSTANT_REPLAY_STATE_COMMAND,
            size: PARAM_VALUE_SIZE,
            data: &raw mut value,
        };
        // SAFETY: `args` matches SESSION_PARAM_ARGS_VERSION and `value` outlives the call.
        let result = unsafe {
            (self.method(GET_SESSION_PARAM_SLOT))(self.interface, (&raw mut args).cast())
        };

        if result == 0 {
            Ok(value.result != 0)
        } else {
            Err(ApiError::Call {
                name: INSTANT_REPLAY_STATE.to_owned(),
                result,
            })
        }
    }
}

/// Drop the reference `LoadLibraryExW` took on a library that yielded no interface.
fn unload(module: *mut c_void) {
    // SAFETY: `module` came from LoadLibraryExW and nothing obtained from it is kept.
    unsafe { FreeLibrary(module) };
}

fn library_path() -> Result<PathBuf, ApiError> {
    let registered = RegKey::predef(HKEY_LOCAL_MACHINE)
        .open_subkey_with_flags(OVERLAY_KEY, KEY_READ | KEY_WOW64_64KEY)
        .and_then(|key| key.get_value::<String, _>(OVERLAY_INSTALL_PATH))
        .ok()
        .map(PathBuf::from);
    let program_files = std::env::var_os(PROGRAM_FILES).map(PathBuf::from);

    registered
        .into_iter()
        .chain(program_files.iter().flat_map(|base| {
            INSTALL_DIRECTORIES
                .iter()
                .map(move |directory| base.join(directory))
        }))
        .map(|directory| directory.join(LIBRARY))
        .find(|path| path.is_file())
        .ok_or(ApiError::NotFound)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn property_names_are_short_ascii_and_nul_padded() {
        let args = PropertyArgs::new(TEMPORARY_PATH).unwrap();

        assert_eq!(args.version, PROPERTY_ARGS_VERSION);
        assert_eq!(
            &args.name[..TEMPORARY_PATH.len()],
            TEMPORARY_PATH.as_bytes()
        );
        assert!(
            args.name[TEMPORARY_PATH.len()..]
                .iter()
                .all(|&byte| byte == 0)
        );
        assert_eq!(args.value.kind, 0);

        assert!(matches!(PropertyArgs::new(""), Err(ApiError::Name)));
        assert!(matches!(PropertyArgs::new("Tëmp"), Err(ApiError::Name)));
        assert!(matches!(
            PropertyArgs::new(&"x".repeat(PROPERTY_NAME_CAPACITY)),
            Err(ApiError::Name)
        ));
    }

    #[test]
    fn only_bstr_variants_carry_text() {
        assert!(Variant::EMPTY.text().is_none());

        let null_bstr = Variant {
            kind: VT_BSTR,
            ..Variant::EMPTY
        };
        assert!(null_bstr.text().is_none());
    }

    #[test]
    fn hresults_are_shown_as_unsigned_hex() {
        let error = ApiError::Call {
            name: TEMPORARY_PATH.to_owned(),
            result: 0x8007_0057_u32.cast_signed(),
        };

        assert!(error.to_string().contains("0x80070057"));
    }

    #[test]
    fn only_failures_to_reach_the_engine_count_as_connection_failures() {
        let call = ApiError::Call {
            name: TEMPORARY_PATH.to_owned(),
            result: 0x8000_4005_u32.cast_signed(),
        };
        let load = ApiError::Load {
            path: PathBuf::from(LIBRARY),
            source: io::Error::from_raw_os_error(5),
        };
        for error in [call, load, ApiError::Create(-1)] {
            assert!(error.is_connection_failure(), "{error}");
        }

        for error in [
            ApiError::NotFound,
            ApiError::Export,
            ApiError::NoText(TEMPORARY_PATH.to_owned()),
            ApiError::Name,
            ApiError::Memory,
            ApiError::Lock,
        ] {
            assert!(!error.is_connection_failure(), "{error}");
        }
    }
}
