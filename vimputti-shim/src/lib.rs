#![feature(c_variadic)]

use lazy_static::lazy_static;
use libc::{c_char, c_int, c_uint};
use std::ffi::{CStr, CString};
use std::os::raw::c_long;
use std::path::PathBuf;
use std::sync::Mutex;
use tracing::{debug, info, warn};

mod path_redirect;
mod syscalls;

use path_redirect::PathRedirector;

// Global state
lazy_static! {
    static ref PATH_REDIRECTOR: PathRedirector = PathRedirector::new();
    static ref ORIGINAL_FUNCTIONS: Mutex<OriginalFunctions> = Mutex::new(OriginalFunctions::new());
}

// Store original function pointers
struct OriginalFunctions {
    open: Option<unsafe extern "C" fn(*const c_char, c_int, ...) -> c_int>,
    open64: Option<unsafe extern "C" fn(*const c_char, c_int, ...) -> c_int>,
    openat: Option<unsafe extern "C" fn(c_int, *const c_char, c_int, ...) -> c_int>,
    openat64: Option<unsafe extern "C" fn(c_int, *const c_char, c_int, ...) -> c_int>,
    access: Option<unsafe extern "C" fn(*const c_char, c_int) -> c_int>,
    stat: Option<unsafe extern "C" fn(*const c_char, *mut libc::stat) -> c_int>,
    lstat: Option<unsafe extern "C" fn(*const c_char, *mut libc::stat) -> c_int>,
    readlink:
        Option<unsafe extern "C" fn(*const c_char, *mut c_char, libc::size_t) -> libc::ssize_t>,
    ioctl: Option<unsafe extern "C" fn(c_int, c_long, ...) -> c_int>,
    close: Option<unsafe extern "C" fn(c_int) -> c_int>,
    fopen: Option<unsafe extern "C" fn(*const c_char, *const c_char) -> *mut libc::FILE>,
    fopen64: Option<unsafe extern "C" fn(*const c_char, *const c_char) -> *mut libc::FILE>,
    opendir: Option<unsafe extern "C" fn(*const c_char) -> *mut libc::DIR>,
    scandir: Option<
        unsafe extern "C" fn(
            *const c_char,
            *mut *mut *mut libc::dirent,
            Option<unsafe extern "C" fn(*const libc::dirent) -> c_int>,
            Option<
                unsafe extern "C" fn(*mut *const libc::dirent, *mut *const libc::dirent) -> c_int,
            >,
        ) -> c_int,
    >,
}

impl OriginalFunctions {
    fn new() -> Self {
        unsafe {
            Self {
                open: Self::get_original("open"),
                open64: Self::get_original("open64"),
                openat: Self::get_original("openat"),
                openat64: Self::get_original("openat64"),
                access: Self::get_original("access"),
                stat: Self::get_original("stat"),
                lstat: Self::get_original("lstat"),
                readlink: Self::get_original("readlink"),
                ioctl: Self::get_original("ioctl"),
                close: Self::get_original("close"),
                fopen: Self::get_original("fopen"),
                fopen64: Self::get_original("fopen64"),
                opendir: Self::get_original("opendir"),
                scandir: Self::get_original("scandir"),
            }
        }
    }

    unsafe fn get_original<T>(name: &str) -> Option<T> {
        let name_cstr = CString::new(name).ok()?;
        let ptr = unsafe { libc::dlsym(libc::RTLD_NEXT, name_cstr.as_ptr()) };
        if ptr.is_null() {
            None
        } else {
            Some(unsafe { std::mem::transmute_copy(&ptr) })
        }
    }
}

// Initialize the shim when loaded
#[ctor::ctor]
fn init_shim() {
    // Initialize tracing
    tracing_subscriber::fmt()
        .with_env_filter(
            tracing_subscriber::EnvFilter::try_from_default_env()
                .unwrap_or_else(|_| tracing_subscriber::EnvFilter::new("warn")),
        )
        .with_target(false)
        .init();

    info!("Vimputti shim loaded!");

    // Detect vimputti base path from environment or use default
    let base_path = std::env::var("VIMPUTTI_PATH").unwrap_or_else(|_| {
        let uid = unsafe { libc::getuid() };
        format!("/run/user/{}/vimputti", uid)
    });

    info!("Vimputti base path: {}", base_path);
}

// =============================================================================
// Intercepted functions
// =============================================================================

/// Intercept open() - redirect paths and handle device nodes
#[unsafe(no_mangle)]
pub unsafe extern "C" fn open(pathname: *const c_char, flags: c_int, mut args: ...) -> c_int {
    if pathname.is_null() {
        return -1;
    }

    let path_str = match unsafe { CStr::from_ptr(pathname).to_str() } {
        Ok(s) => s,
        Err(_) => {
            // Invalid UTF-8, pass through
            let mode: c_uint = unsafe { args.arg() };
            let orig = ORIGINAL_FUNCTIONS.lock().unwrap();
            return unsafe { orig.open.unwrap()(pathname, flags, mode) };
        }
    };

    // Check if this path should be redirected
    if let Some(redirected) = PATH_REDIRECTOR.redirect(path_str) {
        debug!("open: {} -> {}", path_str, redirected);

        // Check if this is a device node we need to handle specially
        if path_str.starts_with("/dev/input/event") {
            return syscalls::open_device_node(&redirected, flags);
        }

        // Regular file redirection
        let new_path = CString::new(redirected).unwrap();
        let mode: c_uint = unsafe { args.arg() };
        let orig = ORIGINAL_FUNCTIONS.lock().unwrap();
        return unsafe { orig.open.unwrap()(new_path.as_ptr(), flags, mode) };
    }

    // Pass through to original open
    let mode: c_uint = unsafe { args.arg() };
    let orig = ORIGINAL_FUNCTIONS.lock().unwrap();
    unsafe { orig.open.unwrap()(pathname, flags, mode) }
}

/// Intercept open64()
#[unsafe(no_mangle)]
pub unsafe extern "C" fn open64(pathname: *const c_char, flags: c_int, mut args: ...) -> c_int {
    if pathname.is_null() {
        return -1;
    }

    let path_str = match unsafe { CStr::from_ptr(pathname).to_str() } {
        Ok(s) => s,
        Err(_) => {
            let mode: c_uint = unsafe { args.arg() };
            let orig = ORIGINAL_FUNCTIONS.lock().unwrap();
            return unsafe { orig.open64.unwrap()(pathname, flags, mode) };
        }
    };

    if let Some(redirected) = PATH_REDIRECTOR.redirect(path_str) {
        debug!("open64: {} -> {}", path_str, redirected);

        if path_str.starts_with("/dev/input/event") {
            return syscalls::open_device_node(&redirected, flags);
        }

        let new_path = CString::new(redirected).unwrap();
        let mode: c_uint = unsafe { args.arg() };
        let orig = ORIGINAL_FUNCTIONS.lock().unwrap();
        return unsafe { orig.open64.unwrap()(new_path.as_ptr(), flags, mode) };
    }

    let mode: c_uint = unsafe { args.arg() };
    let orig = ORIGINAL_FUNCTIONS.lock().unwrap();
    unsafe { orig.open64.unwrap()(pathname, flags, mode) }
}

/// Intercept openat()
#[unsafe(no_mangle)]
pub unsafe extern "C" fn openat(
    dirfd: c_int,
    pathname: *const c_char,
    flags: c_int,
    mut args: ...
) -> c_int {
    if pathname.is_null() {
        return -1;
    }

    let path_str = match unsafe { CStr::from_ptr(pathname).to_str() } {
        Ok(s) => s,
        Err(_) => {
            let mode: c_uint = unsafe { args.arg() };
            let orig = ORIGINAL_FUNCTIONS.lock().unwrap();
            return unsafe { orig.openat.unwrap()(dirfd, pathname, flags, mode) };
        }
    };

    // Only redirect absolute paths
    if path_str.starts_with('/') {
        if let Some(redirected) = PATH_REDIRECTOR.redirect(path_str) {
            debug!("openat: {} -> {}", path_str, redirected);

            if path_str.starts_with("/dev/input/event") {
                return syscalls::open_device_node(&redirected, flags);
            }

            let new_path = CString::new(redirected).unwrap();
            let mode: c_uint = unsafe { args.arg() };
            let orig = ORIGINAL_FUNCTIONS.lock().unwrap();
            return unsafe { orig.openat.unwrap()(dirfd, new_path.as_ptr(), flags, mode) };
        }
    }

    let mode: c_uint = unsafe { args.arg() };
    let orig = ORIGINAL_FUNCTIONS.lock().unwrap();
    unsafe { orig.openat.unwrap()(dirfd, pathname, flags, mode) }
}

/// Intercept openat64()
#[unsafe(no_mangle)]
pub unsafe extern "C" fn openat64(
    dirfd: c_int,
    pathname: *const c_char,
    flags: c_int,
    mut args: ...
) -> c_int {
    if pathname.is_null() {
        return -1;
    }

    let path_str = match unsafe { CStr::from_ptr(pathname).to_str() } {
        Ok(s) => s,
        Err(_) => {
            let mode: c_uint = unsafe { args.arg() };
            let orig = ORIGINAL_FUNCTIONS.lock().unwrap();
            return unsafe { orig.openat64.unwrap()(dirfd, pathname, flags, mode) };
        }
    };

    if path_str.starts_with('/') {
        if let Some(redirected) = PATH_REDIRECTOR.redirect(path_str) {
            debug!("openat64: {} -> {}", path_str, redirected);

            if path_str.starts_with("/dev/input/event") {
                return syscalls::open_device_node(&redirected, flags);
            }

            let new_path = CString::new(redirected).unwrap();
            let mode: c_uint = unsafe { args.arg() };
            let orig = ORIGINAL_FUNCTIONS.lock().unwrap();
            return unsafe { orig.openat64.unwrap()(dirfd, new_path.as_ptr(), flags, mode) };
        }
    }

    let mode: c_uint = unsafe { args.arg() };
    let orig = ORIGINAL_FUNCTIONS.lock().unwrap();
    unsafe { orig.openat64.unwrap()(dirfd, pathname, flags, mode) }
}

/// Intercept access() - check file existence
#[unsafe(no_mangle)]
pub unsafe extern "C" fn access(pathname: *const c_char, mode: c_int) -> c_int {
    if pathname.is_null() {
        return -1;
    }

    let path_str = match unsafe { CStr::from_ptr(pathname).to_str() } {
        Ok(s) => s,
        Err(_) => {
            let orig = ORIGINAL_FUNCTIONS.lock().unwrap();
            return unsafe { orig.access.unwrap()(pathname, mode) };
        }
    };

    if let Some(redirected) = PATH_REDIRECTOR.redirect(path_str) {
        debug!("access: {} -> {}", path_str, redirected);
        let new_path = CString::new(redirected).unwrap();
        let orig = ORIGINAL_FUNCTIONS.lock().unwrap();
        return unsafe { orig.access.unwrap()(new_path.as_ptr(), mode) };
    }

    let orig = ORIGINAL_FUNCTIONS.lock().unwrap();
    unsafe { orig.access.unwrap()(pathname, mode) }
}

/// Intercept stat()
#[unsafe(no_mangle)]
pub unsafe extern "C" fn stat(pathname: *const c_char, statbuf: *mut libc::stat) -> c_int {
    if pathname.is_null() || statbuf.is_null() {
        return -1;
    }

    let path_str = match unsafe { CStr::from_ptr(pathname).to_str() } {
        Ok(s) => s,
        Err(_) => {
            let orig = ORIGINAL_FUNCTIONS.lock().unwrap();
            return unsafe { orig.stat.unwrap()(pathname, statbuf) };
        }
    };

    if let Some(redirected) = PATH_REDIRECTOR.redirect(path_str) {
        debug!("stat: {} -> {}", path_str, redirected);
        let new_path = CString::new(redirected).unwrap();
        let orig = ORIGINAL_FUNCTIONS.lock().unwrap();
        return unsafe { orig.stat.unwrap()(new_path.as_ptr(), statbuf) };
    }

    let orig = ORIGINAL_FUNCTIONS.lock().unwrap();
    unsafe { orig.stat.unwrap()(pathname, statbuf) }
}

/// Intercept lstat()
#[unsafe(no_mangle)]
pub unsafe extern "C" fn lstat(pathname: *const c_char, statbuf: *mut libc::stat) -> c_int {
    if pathname.is_null() || statbuf.is_null() {
        return -1;
    }

    let path_str = match unsafe { CStr::from_ptr(pathname).to_str() } {
        Ok(s) => s,
        Err(_) => {
            let orig = ORIGINAL_FUNCTIONS.lock().unwrap();
            return unsafe { orig.lstat.unwrap()(pathname, statbuf) };
        }
    };

    if let Some(redirected) = PATH_REDIRECTOR.redirect(path_str) {
        debug!("lstat: {} -> {}", path_str, redirected);
        let new_path = CString::new(redirected).unwrap();
        let orig = ORIGINAL_FUNCTIONS.lock().unwrap();
        return unsafe { orig.lstat.unwrap()(new_path.as_ptr(), statbuf) };
    }

    let orig = ORIGINAL_FUNCTIONS.lock().unwrap();
    unsafe { orig.lstat.unwrap()(pathname, statbuf) }
}

/// Intercept readlink()
#[unsafe(no_mangle)]
pub unsafe extern "C" fn readlink(
    pathname: *const c_char,
    buf: *mut c_char,
    bufsiz: libc::size_t,
) -> libc::ssize_t {
    if pathname.is_null() || buf.is_null() {
        return -1;
    }

    let path_str = match unsafe { CStr::from_ptr(pathname).to_str() } {
        Ok(s) => s,
        Err(_) => {
            let orig = ORIGINAL_FUNCTIONS.lock().unwrap();
            return unsafe { orig.readlink.unwrap()(pathname, buf, bufsiz) };
        }
    };

    if let Some(redirected) = PATH_REDIRECTOR.redirect(path_str) {
        debug!("readlink: {} -> {}", path_str, redirected);
        let new_path = CString::new(redirected).unwrap();
        let orig = ORIGINAL_FUNCTIONS.lock().unwrap();
        return unsafe { orig.readlink.unwrap()(new_path.as_ptr(), buf, bufsiz) };
    }

    let orig = ORIGINAL_FUNCTIONS.lock().unwrap();
    unsafe { orig.readlink.unwrap()(pathname, buf, bufsiz) }
}

/// Intercept ioctl() - handle device capability queries
#[unsafe(no_mangle)]
pub unsafe extern "C" fn ioctl(fd: c_int, request: c_long, mut args: ...) -> c_int {
    // Check if this is one of our virtual device FDs
    if syscalls::is_virtual_device_fd(fd) {
        return unsafe { syscalls::handle_ioctl(fd, request as u32, &mut args) };
    }

    // Pass through to original ioctl
    let orig = ORIGINAL_FUNCTIONS.lock().unwrap();
    unsafe { orig.ioctl.unwrap()(fd, request, args) }
}

/// Intercept close() to track FD cleanup
#[unsafe(no_mangle)]
pub unsafe extern "C" fn close(fd: c_int) -> c_int {
    // Clean up our tracking if this was a virtual device FD
    if syscalls::is_virtual_device_fd(fd) {
        syscalls::close_virtual_device(fd);
    }

    // Call the real close
    let orig = ORIGINAL_FUNCTIONS.lock().unwrap();
    unsafe { orig.close.unwrap()(fd) }
}

/// Intercept fopen() - for sysfs file access
#[unsafe(no_mangle)]
pub unsafe extern "C" fn fopen(pathname: *const c_char, mode: *const c_char) -> *mut libc::FILE {
    if pathname.is_null() {
        return std::ptr::null_mut();
    }

    let path_str = match unsafe { CStr::from_ptr(pathname).to_str() } {
        Ok(s) => s,
        Err(_) => {
            // Pass through to original
            let orig = ORIGINAL_FUNCTIONS.lock().unwrap();
            if let Some(orig_fopen) = orig.fopen {
                return unsafe { orig_fopen(pathname, mode) };
            }
            return std::ptr::null_mut();
        }
    };

    if let Some(redirected) = PATH_REDIRECTOR.redirect(path_str) {
        debug!("fopen: {} -> {}", path_str, redirected);

        let new_path_cstring = match CString::new(redirected.clone()) {
            Ok(s) => s,
            Err(e) => {
                warn!("Failed to create CString for {}: {}", redirected, e);
                return std::ptr::null_mut();
            }
        };

        // Call original fopen with redirected path
        let orig = ORIGINAL_FUNCTIONS.lock().unwrap();
        if let Some(orig_fopen) = orig.fopen {
            let result = unsafe { orig_fopen(new_path_cstring.as_ptr(), mode) };
            if result.is_null() {
                debug!("fopen failed for redirected path: {}", redirected);
            } else {
                debug!("fopen succeeded for redirected path: {}", redirected);
            }
            return result;
        }

        return std::ptr::null_mut();
    }

    // Pass through to original
    let orig = ORIGINAL_FUNCTIONS.lock().unwrap();
    if let Some(orig_fopen) = orig.fopen {
        return unsafe { orig_fopen(pathname, mode) };
    }
    std::ptr::null_mut()
}

/// Intercept fopen64()
#[unsafe(no_mangle)]
pub unsafe extern "C" fn fopen64(pathname: *const c_char, mode: *const c_char) -> *mut libc::FILE {
    if pathname.is_null() {
        return std::ptr::null_mut();
    }

    let path_str = match unsafe { CStr::from_ptr(pathname).to_str() } {
        Ok(s) => s,
        Err(_) => {
            let orig = ORIGINAL_FUNCTIONS.lock().unwrap();
            if let Some(orig_fopen64) = orig.fopen64 {
                return unsafe { orig_fopen64(pathname, mode) };
            }
            return std::ptr::null_mut();
        }
    };

    if let Some(redirected) = PATH_REDIRECTOR.redirect(path_str) {
        debug!("fopen64: {} -> {}", path_str, redirected);

        let new_path_cstring = match CString::new(redirected.clone()) {
            Ok(s) => s,
            Err(e) => {
                warn!("Failed to create CString for {}: {}", redirected, e);
                return std::ptr::null_mut();
            }
        };

        let orig = ORIGINAL_FUNCTIONS.lock().unwrap();
        if let Some(orig_fopen64) = orig.fopen64 {
            let result = unsafe { orig_fopen64(new_path_cstring.as_ptr(), mode) };
            if result.is_null() {
                debug!("fopen64 failed for redirected path: {}", redirected);
            } else {
                debug!("fopen64 succeeded for redirected path: {}", redirected);
            }
            return result;
        }
        return std::ptr::null_mut();
    }

    let orig = ORIGINAL_FUNCTIONS.lock().unwrap();
    if let Some(orig_fopen64) = orig.fopen64 {
        return unsafe { orig_fopen64(pathname, mode) };
    }
    std::ptr::null_mut()
}

/// Intercept opendir() to fake /dev/input directory
#[unsafe(no_mangle)]
pub unsafe extern "C" fn opendir(name: *const c_char) -> *mut libc::DIR {
    if name.is_null() {
        return std::ptr::null_mut();
    }

    let path_str = match unsafe { CStr::from_ptr(name).to_str() } {
        Ok(s) => s,
        Err(_) => {
            let orig = ORIGINAL_FUNCTIONS.lock().unwrap();
            if let Some(orig_opendir) = orig.opendir {
                return unsafe { orig_opendir(name) };
            }
            return std::ptr::null_mut();
        }
    };

    // Redirect /dev/input to our devices directory
    if path_str == "/dev/input" {
        let redirected = PATH_REDIRECTOR
            .redirect("/dev/input/event0")
            .map(|p| {
                // Extract base path (remove the event0 part)
                PathBuf::from(p).parent().unwrap().to_path_buf()
            })
            .unwrap_or_else(|| PathBuf::from("/dev/input"));

        debug!("opendir: /dev/input -> {}", redirected.display());
        let new_path = CString::new(redirected.to_string_lossy().as_ref()).unwrap();
        let orig = ORIGINAL_FUNCTIONS.lock().unwrap();
        if let Some(orig_opendir) = orig.opendir {
            return unsafe { orig_opendir(new_path.as_ptr()) };
        }
        return std::ptr::null_mut();
    }

    // Check for other redirections
    if let Some(redirected) = PATH_REDIRECTOR.redirect(path_str) {
        debug!("opendir: {} -> {}", path_str, redirected);
        let new_path = CString::new(redirected).unwrap();
        let orig = ORIGINAL_FUNCTIONS.lock().unwrap();
        if let Some(orig_opendir) = orig.opendir {
            return unsafe { orig_opendir(new_path.as_ptr()) };
        }
        return std::ptr::null_mut();
    }

    let orig = ORIGINAL_FUNCTIONS.lock().unwrap();
    if let Some(orig_opendir) = orig.opendir {
        return unsafe { orig_opendir(name) };
    }
    std::ptr::null_mut()
}

/// Intercept scandir() for directory enumeration
#[unsafe(no_mangle)]
pub unsafe extern "C" fn scandir(
    dirp: *const c_char,
    namelist: *mut *mut *mut libc::dirent,
    filter: Option<unsafe extern "C" fn(*const libc::dirent) -> c_int>,
    compar: Option<
        unsafe extern "C" fn(*mut *const libc::dirent, *mut *const libc::dirent) -> c_int,
    >,
) -> c_int {
    if dirp.is_null() {
        return -1;
    }

    let path_str = match unsafe { CStr::from_ptr(dirp).to_str() } {
        Ok(s) => s,
        Err(_) => {
            let orig = ORIGINAL_FUNCTIONS.lock().unwrap();
            if let Some(orig_scandir) = orig.scandir {
                return unsafe { orig_scandir(dirp, namelist, filter, compar) };
            }
            return -1;
        }
    };

    // Redirect /dev/input
    if path_str == "/dev/input" {
        let redirected = PATH_REDIRECTOR
            .redirect("/dev/input/event0")
            .map(|p| PathBuf::from(p).parent().unwrap().to_path_buf())
            .unwrap_or_else(|| PathBuf::from("/dev/input"));

        debug!("scandir: /dev/input -> {}", redirected.display());
        let new_path = CString::new(redirected.to_string_lossy().as_ref()).unwrap();
        let orig = ORIGINAL_FUNCTIONS.lock().unwrap();
        if let Some(orig_scandir) = orig.scandir {
            return unsafe { orig_scandir(new_path.as_ptr(), namelist, filter, compar) };
        }
        return -1;
    }

    // Check for other redirections
    if let Some(redirected) = PATH_REDIRECTOR.redirect(path_str) {
        debug!("scandir: {} -> {}", path_str, redirected);
        let new_path = CString::new(redirected).unwrap();
        let orig = ORIGINAL_FUNCTIONS.lock().unwrap();
        if let Some(orig_scandir) = orig.scandir {
            return unsafe { orig_scandir(new_path.as_ptr(), namelist, filter, compar) };
        }
        return -1;
    }

    let orig = ORIGINAL_FUNCTIONS.lock().unwrap();
    if let Some(orig_scandir) = orig.scandir {
        return unsafe { orig_scandir(dirp, namelist, filter, compar) };
    }
    -1
}
