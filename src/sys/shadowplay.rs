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
/// The first-generation `IShadowPlayApi` table; its first eight methods are used here.
const INTERFACE_VERSION: u32 = 0x0001_0008;
/// Client 6 joins the message bus as `ShadowPlayApi_TestingTool`. The library accepts 3
/// through 11; 5 is the overlay itself, and a second registration under its name is dropped.
const CLIENT: u32 = 6;
/// Low 16 bits: `size_of::<PropertyArgs>()`.
const PROPERTY_ARGS_VERSION: u32 = 0x0001_0060;
const PROPERTY_NAME_CAPACITY: usize = 64;
const SET_PROPERTY_SLOT: usize = 6;
const GET_PROPERTY_SLOT: usize = 7;
const VT_BSTR: u16 = 8;

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

#[link(name = "kernel32")]
unsafe extern "system" {
    fn LoadLibraryExW(file_name: *const u16, file: *mut c_void, flags: u32) -> *mut c_void;
    fn GetProcAddress(module: *mut c_void, name: *const u8) -> Option<unsafe extern "system" fn()>;
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

const _: () = assert!(size_of::<CreateParams>() == 0x18);
const _: () = assert!(size_of::<Variant>() == 24);
const _: () = assert!(size_of::<PropertyArgs>() == 0x60);

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
        if name.is_empty() || !name.is_ascii() || name.len() >= PROPERTY_NAME_CAPACITY {
            return Err(ApiError::Name);
        }

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

        // SAFETY: `module` is a loaded library and the export name is NUL-terminated.
        let Some(export) = (unsafe { GetProcAddress(module, CREATE_INTERFACE.as_ptr()) }) else {
            return Err(ApiError::Export);
        };
        // SAFETY: the export is the library's documented factory function.
        let create =
            unsafe { std::mem::transmute::<unsafe extern "system" fn(), CreateInterface>(export) };

        let mut interface: *mut c_void = ptr::null_mut();
        let mut params = CreateParams {
            version: CREATE_PARAMS_VERSION,
            interface_version: INTERFACE_VERSION,
            client: CLIENT,
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
mod tests;
