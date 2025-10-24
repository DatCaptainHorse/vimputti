#![feature(c_variadic)]

use lazy_static::lazy_static;
use libc::{c_char, c_int, c_uint, c_void};
use std::ffi::{CStr, CString};
use std::os::raw::c_long;
use std::path::PathBuf;
use tracing::debug;

mod libudev;
mod path_redirect;
mod syscalls;

use path_redirect::PathRedirector;

// Global state
lazy_static! {
    static ref PATH_REDIRECTOR: PathRedirector = PathRedirector::new();
    static ref ORIGINAL_FUNCTIONS: OriginalFunctions = OriginalFunctions::new();
}

// Store original function pointers
struct OriginalFunctions {
    getuid: Option<unsafe extern "C" fn() -> libc::uid_t>,
    open: Option<unsafe extern "C" fn(*const c_char, c_int, ...) -> c_int>,
    open64: Option<unsafe extern "C" fn(*const c_char, c_int, ...) -> c_int>,
    openat: Option<unsafe extern "C" fn(c_int, *const c_char, c_int, ...) -> c_int>,
    openat64: Option<unsafe extern "C" fn(c_int, *const c_char, c_int, ...) -> c_int>,
    access: Option<unsafe extern "C" fn(*const c_char, c_int) -> c_int>,
    stat: Option<unsafe extern "C" fn(*const c_char, *mut libc::stat) -> c_int>,
    stat64: Option<unsafe extern "C" fn(*const c_char, *mut libc::stat64) -> c_int>,
    lstat: Option<unsafe extern "C" fn(*const c_char, *mut libc::stat) -> c_int>,
    lstat64: Option<unsafe extern "C" fn(*const c_char, *mut libc::stat64) -> c_int>,
    xstat: Option<unsafe extern "C" fn(c_int, *const c_char, *mut libc::stat) -> c_int>,
    xstat64: Option<unsafe extern "C" fn(c_int, *const c_char, *mut libc::stat64) -> c_int>,
    lxstat: Option<unsafe extern "C" fn(c_int, *const c_char, *mut libc::stat) -> c_int>,
    lxstat64: Option<unsafe extern "C" fn(c_int, *const c_char, *mut libc::stat64) -> c_int>,
    readlink:
        Option<unsafe extern "C" fn(*const c_char, *mut c_char, libc::size_t) -> libc::ssize_t>,
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
    fdopendir: Option<unsafe extern "C" fn(c_int) -> *mut libc::DIR>,
    readdir: Option<unsafe extern "C" fn(*mut libc::DIR) -> *mut libc::dirent>,
    readdir64: Option<unsafe extern "C" fn(*mut libc::DIR) -> *mut libc::dirent64>,
    ioctl: Option<unsafe extern "C" fn(c_int, c_long, ...) -> c_int>,
    read: Option<unsafe extern "C" fn(c_int, *mut c_void, libc::size_t) -> libc::ssize_t>,
    write: Option<unsafe extern "C" fn(c_int, *const c_void, libc::size_t) -> libc::ssize_t>,
    poll: Option<unsafe extern "C" fn(*mut libc::pollfd, libc::nfds_t, c_int) -> c_int>,
    epoll_wait: Option<unsafe extern "C" fn(c_int, *mut libc::epoll_event, c_int, c_int) -> c_int>,
    epoll_pwait: Option<
        unsafe extern "C" fn(
            c_int,
            *mut libc::epoll_event,
            c_int,
            c_int,
            *const libc::sigset_t,
        ) -> c_int,
    >,
    inotify_init: Option<unsafe extern "C" fn() -> c_int>,
    inotify_init1: Option<unsafe extern "C" fn(c_int) -> c_int>,
    inotify_add_watch: Option<unsafe extern "C" fn(c_int, *const c_char, u32) -> c_int>,
    socket: Option<unsafe extern "C" fn(c_int, c_int, c_int) -> c_int>,
    connect: Option<unsafe extern "C" fn(c_int, *const libc::sockaddr, libc::socklen_t) -> c_int>,
    bind: Option<unsafe extern "C" fn(c_int, *const libc::sockaddr, libc::socklen_t) -> c_int>,
}
impl OriginalFunctions {
    fn new() -> Self {
        unsafe {
            Self {
                getuid: Self::get_original("getuid"),
                open: Self::get_original("open"),
                open64: Self::get_original("open64"),
                openat: Self::get_original("openat"),
                openat64: Self::get_original("openat64"),
                access: Self::get_original("access"),
                stat: Self::get_original("stat"),
                stat64: Self::get_original("stat64"),
                lstat: Self::get_original("lstat"),
                lstat64: Self::get_original("lstat64"),
                xstat: Self::get_original("__xstat"),
                xstat64: Self::get_original("__xstat64"),
                lxstat: Self::get_original("__lxstat"),
                lxstat64: Self::get_original("__lxstat64"),
                readlink: Self::get_original("readlink"),
                close: Self::get_original("close"),
                fopen: Self::get_original("fopen"),
                fopen64: Self::get_original("fopen64"),
                opendir: Self::get_original("opendir"),
                scandir: Self::get_original("scandir"),
                fdopendir: Self::get_original("fdopendir"),
                readdir: Self::get_original("readdir"),
                readdir64: Self::get_original("readdir64"),
                ioctl: Self::get_original("ioctl"),
                read: Self::get_original("read"),
                write: Self::get_original("write"),
                poll: Self::get_original("poll"),
                epoll_wait: Self::get_original("epoll_wait"),
                epoll_pwait: Self::get_original("epoll_pwait"),
                inotify_init: Self::get_original("inotify_init"),
                inotify_init1: Self::get_original("inotify_init1"),
                inotify_add_watch: Self::get_original("inotify_add_watch"),
                socket: Self::get_original("socket"),
                connect: Self::get_original("connect"),
                bind: Self::get_original("bind"),
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
        .init();
}

// =============================================================================
// Intercepted functions
// =============================================================================

/// Intercept open() - redirect paths and handle device nodes
#[unsafe(no_mangle)]
pub unsafe extern "C" fn open(pathname: *const c_char, flags: c_int, mut args: ...) -> c_int {
    if pathname.is_null() {
        if let Some(orig_open) = ORIGINAL_FUNCTIONS.open {
            return unsafe { orig_open(pathname, flags, args) };
        }
        return -1;
    }

    let path_str = match unsafe { CStr::from_ptr(pathname).to_str() } {
        Ok(s) => s,
        Err(_) => {
            // Invalid UTF-8, pass through
            let mode: c_uint = unsafe { args.arg() };
            if let Some(orig_open) = ORIGINAL_FUNCTIONS.open {
                return unsafe { orig_open(pathname, flags, mode) };
            }
            return -1;
        }
    };

    // Check if this path should be redirected
    if let Some(redirected) = PATH_REDIRECTOR.redirect(path_str) {
        debug!("open: {} -> {}", path_str, redirected);

        // Check if this is a device node we need to handle specially
        if path_str.contains("/dev/uinput")
            || path_str.starts_with("/dev/input/event")
            || path_str.starts_with("/dev/input/js")
        {
            return syscalls::open_device_node(&redirected, flags);
        }

        // Regular file redirection
        let new_path = CString::new(redirected).unwrap();
        let mode: c_uint = unsafe { args.arg() };
        if let Some(orig_open) = ORIGINAL_FUNCTIONS.open {
            return unsafe { orig_open(new_path.as_ptr(), flags, mode) };
        }
        return -1;
    }

    // Pass through to original open
    let mode: c_uint = unsafe { args.arg() };
    if let Some(orig_open) = ORIGINAL_FUNCTIONS.open {
        return unsafe { orig_open(pathname, flags, mode) };
    }
    -1
}

/// Intercept open64()
#[unsafe(no_mangle)]
pub unsafe extern "C" fn open64(pathname: *const c_char, flags: c_int, mut args: ...) -> c_int {
    if pathname.is_null() {
        if let Some(orig_open64) = ORIGINAL_FUNCTIONS.open64 {
            return unsafe { orig_open64(pathname, flags, args) };
        }
        return -1;
    }

    let path_str = match unsafe { CStr::from_ptr(pathname).to_str() } {
        Ok(s) => s,
        Err(_) => {
            let mode: c_uint = unsafe { args.arg() };
            if let Some(orig_open64) = ORIGINAL_FUNCTIONS.open64 {
                return unsafe { orig_open64(pathname, flags, mode) };
            }
            return -1;
        }
    };

    if let Some(redirected) = PATH_REDIRECTOR.redirect(path_str) {
        debug!("open64: {} -> {}", path_str, redirected);

        if path_str.contains("/dev/uinput")
            || path_str.starts_with("/dev/input/event")
            || path_str.starts_with("/dev/input/js")
        {
            return syscalls::open_device_node(&redirected, flags);
        }

        let new_path = CString::new(redirected).unwrap();
        let mode: c_uint = unsafe { args.arg() };
        if let Some(orig_open64) = ORIGINAL_FUNCTIONS.open64 {
            return unsafe { orig_open64(new_path.as_ptr(), flags, mode) };
        }
        return -1;
    }

    let mode: c_uint = unsafe { args.arg() };
    if let Some(orig_open64) = ORIGINAL_FUNCTIONS.open64 {
        return unsafe { orig_open64(pathname, flags, mode) };
    }
    -1
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
        if let Some(orig_openat) = ORIGINAL_FUNCTIONS.openat {
            return unsafe { orig_openat(dirfd, pathname, flags, args) };
        }
        return -1;
    }

    let path_str = match unsafe { CStr::from_ptr(pathname).to_str() } {
        Ok(s) => s,
        Err(_) => {
            let mode: c_uint = unsafe { args.arg() };
            if let Some(orig_openat) = ORIGINAL_FUNCTIONS.openat {
                return unsafe { orig_openat(dirfd, pathname, flags, mode) };
            }
            return -1;
        }
    };

    // Only redirect absolute paths
    if path_str.starts_with('/') {
        if let Some(redirected) = PATH_REDIRECTOR.redirect(path_str) {
            debug!("openat: {} -> {}", path_str, redirected);

            if path_str.contains("/dev/uinput")
                || path_str.starts_with("/dev/input/event")
                || path_str.starts_with("/dev/input/js")
            {
                return syscalls::open_device_node(&redirected, flags);
            }

            let new_path = CString::new(redirected).unwrap();
            let mode: c_uint = unsafe { args.arg() };
            if let Some(orig_openat) = ORIGINAL_FUNCTIONS.openat {
                return unsafe { orig_openat(dirfd, new_path.as_ptr(), flags, mode) };
            }
            return -1;
        }
    }

    let mode: c_uint = unsafe { args.arg() };
    if let Some(orig_openat) = ORIGINAL_FUNCTIONS.openat {
        return unsafe { orig_openat(dirfd, pathname, flags, mode) };
    }
    -1
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
        if let Some(orig_openat64) = ORIGINAL_FUNCTIONS.openat64 {
            return unsafe { orig_openat64(dirfd, pathname, flags) };
        }
        return -1;
    }

    let path_str = match unsafe { CStr::from_ptr(pathname).to_str() } {
        Ok(s) => s,
        Err(_) => {
            let mode: c_uint = unsafe { args.arg() };
            if let Some(orig_openat64) = ORIGINAL_FUNCTIONS.openat64 {
                return unsafe { orig_openat64(dirfd, pathname, flags, mode) };
            }
            return -1;
        }
    };

    if path_str.starts_with('/') {
        if let Some(redirected) = PATH_REDIRECTOR.redirect(path_str) {
            debug!("openat64: {} -> {}", path_str, redirected);

            if path_str.contains("/dev/uinput")
                || path_str.starts_with("/dev/input/event")
                || path_str.starts_with("/dev/input/js")
            {
                return syscalls::open_device_node(&redirected, flags);
            }

            let new_path = CString::new(redirected).unwrap();
            let mode: c_uint = unsafe { args.arg() };
            if let Some(orig_openat64) = ORIGINAL_FUNCTIONS.openat64 {
                return unsafe { orig_openat64(dirfd, new_path.as_ptr(), flags, mode) };
            }
            return -1;
        }
    }

    let mode: c_uint = unsafe { args.arg() };
    if let Some(orig_openat64) = ORIGINAL_FUNCTIONS.openat64 {
        return unsafe { orig_openat64(dirfd, pathname, flags, mode) };
    }
    -1
}

/// Intercept access() - check file existence
#[unsafe(no_mangle)]
pub unsafe extern "C" fn access(pathname: *const c_char, mode: c_int) -> c_int {
    if pathname.is_null() {
        if let Some(orig_access) = ORIGINAL_FUNCTIONS.access {
            return unsafe { orig_access(pathname, mode) };
        }
        return -1;
    }

    let path_str = match unsafe { CStr::from_ptr(pathname).to_str() } {
        Ok(s) => s,
        Err(_) => {
            if let Some(orig_access) = ORIGINAL_FUNCTIONS.access {
                return unsafe { orig_access(pathname, mode) };
            }
            return -1;
        }
    };

    if let Some(redirected) = PATH_REDIRECTOR.redirect(path_str) {
        debug!("access: {} -> {}", path_str, redirected);
        let new_path = CString::new(redirected).unwrap();
        if let Some(orig_access) = ORIGINAL_FUNCTIONS.access {
            return unsafe { orig_access(new_path.as_ptr(), mode) };
        }
        return -1;
    }

    if let Some(orig_access) = ORIGINAL_FUNCTIONS.access {
        return unsafe { orig_access(pathname, mode) };
    }
    -1
}

/// Intercept stat()
#[unsafe(no_mangle)]
pub unsafe extern "C" fn stat(pathname: *const c_char, statbuf: *mut libc::stat) -> c_int {
    if pathname.is_null() || statbuf.is_null() {
        if let Some(orig) = ORIGINAL_FUNCTIONS.stat {
            return unsafe { orig(pathname, statbuf) };
        }
        return -1;
    }

    let path_str = match unsafe { CStr::from_ptr(pathname).to_str() } {
        Ok(s) => s,
        Err(_) => {
            if let Some(orig_stat) = ORIGINAL_FUNCTIONS.stat {
                return unsafe { orig_stat(pathname, statbuf) };
            }
            return -1;
        }
    };

    if let Some(redirected) = PATH_REDIRECTOR.redirect(path_str) {
        debug!("stat: {} -> {}", path_str, redirected);
        let new_path = CString::new(redirected).unwrap();
        if let Some(orig_stat) = ORIGINAL_FUNCTIONS.stat {
            return unsafe { orig_stat(new_path.as_ptr(), statbuf) };
        }
        return -1;
    }

    if let Some(orig_stat) = ORIGINAL_FUNCTIONS.stat {
        return unsafe { orig_stat(pathname, statbuf) };
    }
    -1
}

/// Intercept lstat()
#[unsafe(no_mangle)]
pub unsafe extern "C" fn lstat(pathname: *const c_char, statbuf: *mut libc::stat) -> c_int {
    if pathname.is_null() || statbuf.is_null() {
        if let Some(orig) = ORIGINAL_FUNCTIONS.lstat {
            return unsafe { orig(pathname, statbuf) };
        }
        return -1;
    }

    let path_str = match unsafe { CStr::from_ptr(pathname).to_str() } {
        Ok(s) => s,
        Err(_) => {
            if let Some(orig_lstat) = ORIGINAL_FUNCTIONS.lstat {
                return unsafe { orig_lstat(pathname, statbuf) };
            }
            return -1;
        }
    };

    if let Some(redirected) = PATH_REDIRECTOR.redirect(path_str) {
        debug!("lstat: {} -> {}", path_str, redirected);
        let new_path = CString::new(redirected).unwrap();
        if let Some(orig_lstat) = ORIGINAL_FUNCTIONS.lstat {
            return unsafe { orig_lstat(new_path.as_ptr(), statbuf) };
        }
        return -1;
    }

    if let Some(orig_lstat) = ORIGINAL_FUNCTIONS.lstat {
        return unsafe { orig_lstat(pathname, statbuf) };
    }
    -1
}

/// Intercept readlink()
#[unsafe(no_mangle)]
pub unsafe extern "C" fn readlink(
    pathname: *const c_char,
    buf: *mut c_char,
    bufsiz: libc::size_t,
) -> libc::ssize_t {
    if pathname.is_null() || buf.is_null() {
        if let Some(orig) = ORIGINAL_FUNCTIONS.readlink {
            return unsafe { orig(pathname, buf, bufsiz) };
        }
        return -1;
    }

    let path_str = match unsafe { CStr::from_ptr(pathname).to_str() } {
        Ok(s) => s,
        Err(_) => {
            if let Some(orig_readlink) = ORIGINAL_FUNCTIONS.readlink {
                return unsafe { orig_readlink(pathname, buf, bufsiz) };
            }
            return -1;
        }
    };

    if let Some(redirected) = PATH_REDIRECTOR.redirect(path_str) {
        debug!("readlink: {} -> {}", path_str, redirected);
        let new_path = CString::new(redirected).unwrap();
        if let Some(orig_readlink) = ORIGINAL_FUNCTIONS.readlink {
            return unsafe { orig_readlink(new_path.as_ptr(), buf, bufsiz) };
        }
        return -1;
    }

    if let Some(orig_readlink) = ORIGINAL_FUNCTIONS.readlink {
        return unsafe { orig_readlink(pathname, buf, bufsiz) };
    }
    -1
}

/// Intercept read() - handle device reads
#[unsafe(no_mangle)]
pub unsafe extern "C" fn read(fd: c_int, buf: *mut c_void, count: libc::size_t) -> libc::ssize_t {
    // Check if this is a uinput emulator FD
    if syscalls::is_uinput_fd(fd) {
        // Return EAGAIN (would block)
        // This tells applications like Steam "no data right now, try again later"
        unsafe {
            *libc::__errno_location() = libc::EAGAIN;
        }
        return -1;
    }

    if let Some(orig_read) = ORIGINAL_FUNCTIONS.read {
        return unsafe { orig_read(fd, buf, count) };
    }
    -1
}

/// Intercept write() - handle uinput event writes
#[unsafe(no_mangle)]
pub unsafe extern "C" fn write(
    fd: c_int,
    buf: *const c_void,
    count: libc::size_t,
) -> libc::ssize_t {
    // Check if this is a uinput emulator FD
    if syscalls::is_uinput_fd(fd) {
        return unsafe { syscalls::handle_uinput_write(fd, buf, count) };
    }

    // Check if this is a virtual device FD
    if syscalls::is_virtual_device_fd(fd) {
        return unsafe { syscalls::handle_virtual_device_write(fd, buf, count) };
    }

    if let Some(orig_write) = ORIGINAL_FUNCTIONS.write {
        return unsafe { orig_write(fd, buf, count) };
    }
    -1
}

/// Intercept ioctl() - handle device capability queries
#[unsafe(no_mangle)]
pub unsafe extern "C" fn ioctl(fd: c_int, request: c_long, mut args: ...) -> c_int {
    // Check if this is a uinput emulator FD
    if syscalls::is_uinput_fd(fd) {
        return unsafe { syscalls::handle_uinput_ioctl(fd, request as u32, &mut args) };
    }

    // Check if this is one of our virtual device FDs
    if syscalls::is_virtual_device_fd(fd) {
        return unsafe { syscalls::handle_ioctl(fd, request as u32, &mut args) };
    }

    // Pass through to original ioctl
    if let Some(orig_ioctl) = ORIGINAL_FUNCTIONS.ioctl {
        let arg: *mut c_void = unsafe { args.arg() };
        return unsafe { orig_ioctl(fd, request, arg) };
    }
    -1
}

/// Intercept fdopendir()
#[unsafe(no_mangle)]
pub unsafe extern "C" fn fdopendir(fd: c_int) -> *mut libc::DIR {
    if let Some(orig) = ORIGINAL_FUNCTIONS.fdopendir {
        return unsafe { orig(fd) };
    }
    std::ptr::null_mut()
}

/// Intercept readdir()
#[unsafe(no_mangle)]
pub unsafe extern "C" fn readdir(dirp: *mut libc::DIR) -> *mut libc::dirent {
    if let Some(orig) = ORIGINAL_FUNCTIONS.readdir {
        return unsafe { orig(dirp) };
    }
    std::ptr::null_mut()
}

/// Intercept readdir64()
#[unsafe(no_mangle)]
pub unsafe extern "C" fn readdir64(dirp: *mut libc::DIR) -> *mut libc::dirent64 {
    if let Some(orig) = ORIGINAL_FUNCTIONS.readdir64 {
        return unsafe { orig(dirp) };
    }
    std::ptr::null_mut()
}

/// Intercept close() to track FD cleanup
#[unsafe(no_mangle)]
pub unsafe extern "C" fn close(fd: c_int) -> c_int {
    // Clean up our tracking if this was a virtual device FD
    if syscalls::is_virtual_device_fd(fd) {
        syscalls::close_virtual_device(fd);
    }

    // Call the real close
    if let Some(orig_close) = ORIGINAL_FUNCTIONS.close {
        return unsafe { orig_close(fd) };
    }
    -1
}

/// Intercept fopen() - for sysfs file access
#[unsafe(no_mangle)]
pub unsafe extern "C" fn fopen(pathname: *const c_char, mode: *const c_char) -> *mut libc::FILE {
    if pathname.is_null() {
        if let Some(orig_fopen) = ORIGINAL_FUNCTIONS.fopen {
            return unsafe { orig_fopen(pathname, mode) };
        }
        return std::ptr::null_mut();
    }

    let path_str = match unsafe { CStr::from_ptr(pathname).to_str() } {
        Ok(s) => s,
        Err(_) => {
            // Pass through to original
            if let Some(orig_fopen) = ORIGINAL_FUNCTIONS.fopen {
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
                debug!("Failed to create CString for {}: {}", redirected, e);
                return std::ptr::null_mut();
            }
        };

        // Call original fopen with redirected path
        if let Some(orig_fopen) = ORIGINAL_FUNCTIONS.fopen {
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
    if let Some(orig_fopen) = ORIGINAL_FUNCTIONS.fopen {
        return unsafe { orig_fopen(pathname, mode) };
    }
    std::ptr::null_mut()
}

/// Intercept fopen64()
#[unsafe(no_mangle)]
pub unsafe extern "C" fn fopen64(pathname: *const c_char, mode: *const c_char) -> *mut libc::FILE {
    if pathname.is_null() {
        if let Some(orig_fopen64) = ORIGINAL_FUNCTIONS.fopen64 {
            return unsafe { orig_fopen64(pathname, mode) };
        }
        return std::ptr::null_mut();
    }

    let path_str = match unsafe { CStr::from_ptr(pathname).to_str() } {
        Ok(s) => s,
        Err(_) => {
            if let Some(orig_fopen64) = ORIGINAL_FUNCTIONS.fopen64 {
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
                debug!("Failed to create CString for {}: {}", redirected, e);
                return std::ptr::null_mut();
            }
        };

        if let Some(orig_fopen64) = ORIGINAL_FUNCTIONS.fopen64 {
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

    if let Some(orig_fopen64) = ORIGINAL_FUNCTIONS.fopen64 {
        return unsafe { orig_fopen64(pathname, mode) };
    }
    std::ptr::null_mut()
}

/// Intercept opendir() to fake /dev/input directory
#[unsafe(no_mangle)]
pub unsafe extern "C" fn opendir(name: *const c_char) -> *mut libc::DIR {
    if name.is_null() {
        if let Some(orig_opendir) = ORIGINAL_FUNCTIONS.opendir {
            return unsafe { orig_opendir(name) };
        }
        return std::ptr::null_mut();
    }

    let path_str = match unsafe { CStr::from_ptr(name).to_str() } {
        Ok(s) => s,
        Err(_) => {
            if let Some(orig_opendir) = ORIGINAL_FUNCTIONS.opendir {
                return unsafe { orig_opendir(name) };
            }
            return std::ptr::null_mut();
        }
    };

    // Redirect /dev/input to our devices directory
    if path_str == "/dev/input" {
        let base_path = syscalls::get_base_path();
        let redirected = PathBuf::from(format!("{}/devices", base_path));

        debug!("opendir: /dev/input -> {}", redirected.display());
        let new_path = CString::new(redirected.to_string_lossy().as_ref()).unwrap();
        if let Some(orig_opendir) = ORIGINAL_FUNCTIONS.opendir {
            return unsafe { orig_opendir(new_path.as_ptr()) };
        }
        return std::ptr::null_mut();
    }

    // Check for other redirections
    if let Some(redirected) = PATH_REDIRECTOR.redirect(path_str) {
        debug!("opendir: {} -> {}", path_str, redirected);
        let new_path = CString::new(redirected).unwrap();
        if let Some(orig_opendir) = ORIGINAL_FUNCTIONS.opendir {
            return unsafe { orig_opendir(new_path.as_ptr()) };
        }
        return std::ptr::null_mut();
    }

    if let Some(orig_opendir) = ORIGINAL_FUNCTIONS.opendir {
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
        if let Some(orig_scandir) = ORIGINAL_FUNCTIONS.scandir {
            return unsafe { orig_scandir(dirp, namelist, filter, compar) };
        }
        return -1;
    }

    let path_str = match unsafe { CStr::from_ptr(dirp).to_str() } {
        Ok(s) => s,
        Err(_) => {
            if let Some(orig_scandir) = ORIGINAL_FUNCTIONS.scandir {
                return unsafe { orig_scandir(dirp, namelist, filter, compar) };
            }
            return -1;
        }
    };

    // Redirect /dev/input
    if path_str == "/dev/input" {
        let base_path = syscalls::get_base_path();
        let redirected = PathBuf::from(format!("{}/devices", base_path));

        debug!("scandir: /dev/input -> {}", redirected.display());
        let new_path = CString::new(redirected.to_string_lossy().as_ref()).unwrap();
        if let Some(orig_scandir) = ORIGINAL_FUNCTIONS.scandir {
            return unsafe { orig_scandir(new_path.as_ptr(), namelist, filter, compar) };
        }
        return -1;
    }

    // Check for other redirections
    if let Some(redirected) = PATH_REDIRECTOR.redirect(path_str) {
        debug!("scandir: {} -> {}", path_str, redirected);
        let new_path = CString::new(redirected).unwrap();
        if let Some(orig_scandir) = ORIGINAL_FUNCTIONS.scandir {
            return unsafe { orig_scandir(new_path.as_ptr(), namelist, filter, compar) };
        }
        return -1;
    }

    if let Some(orig_scandir) = ORIGINAL_FUNCTIONS.scandir {
        return unsafe { orig_scandir(dirp, namelist, filter, compar) };
    }
    -1
}

/// Intercept stat64()
#[unsafe(no_mangle)]
pub unsafe extern "C" fn stat64(pathname: *const c_char, statbuf: *mut libc::stat64) -> c_int {
    if pathname.is_null() || statbuf.is_null() {
        if let Some(orig_stat64) = ORIGINAL_FUNCTIONS.stat64 {
            return unsafe { orig_stat64(pathname, statbuf) };
        }
        return -1;
    }

    let path_str = match unsafe { CStr::from_ptr(pathname).to_str() } {
        Ok(s) => s,
        Err(_) => {
            if let Some(orig_stat64) = ORIGINAL_FUNCTIONS.stat64 {
                return unsafe { orig_stat64(pathname, statbuf) };
            }
            return -1;
        }
    };

    if let Some(redirected) = PATH_REDIRECTOR.redirect(path_str) {
        debug!("stat64: {} -> {}", path_str, redirected);
        let new_path = CString::new(redirected).unwrap();
        if let Some(orig_stat64) = ORIGINAL_FUNCTIONS.stat64 {
            return unsafe { orig_stat64(new_path.as_ptr(), statbuf) };
        }
    }

    if let Some(orig_stat64) = ORIGINAL_FUNCTIONS.stat64 {
        return unsafe { orig_stat64(pathname, statbuf) };
    }
    -1
}

/// Intercept lstat64()
#[unsafe(no_mangle)]
pub unsafe extern "C" fn lstat64(pathname: *const c_char, statbuf: *mut libc::stat64) -> c_int {
    if pathname.is_null() || statbuf.is_null() {
        if let Some(orig) = ORIGINAL_FUNCTIONS.lstat64 {
            return unsafe { orig(pathname, statbuf) };
        }
        return -1;
    }

    let path_str = match unsafe { CStr::from_ptr(pathname).to_str() } {
        Ok(s) => s,
        Err(_) => {
            if let Some(orig_lstat64) = ORIGINAL_FUNCTIONS.lstat64 {
                return unsafe { orig_lstat64(pathname, statbuf) };
            }
            return -1;
        }
    };

    if let Some(redirected) = PATH_REDIRECTOR.redirect(path_str) {
        debug!("lstat64: {} -> {}", path_str, redirected);
        let new_path = CString::new(redirected).unwrap();
        if let Some(orig_lstat64) = ORIGINAL_FUNCTIONS.lstat64 {
            return unsafe { orig_lstat64(new_path.as_ptr(), statbuf) };
        }
    }

    if let Some(orig_lstat64) = ORIGINAL_FUNCTIONS.lstat64 {
        return unsafe { orig_lstat64(pathname, statbuf) };
    }
    -1
}

/// Intercept __xstat (glibc wrapper for stat)
#[unsafe(no_mangle)]
pub unsafe extern "C" fn __xstat(
    ver: c_int,
    pathname: *const c_char,
    statbuf: *mut libc::stat,
) -> c_int {
    if pathname.is_null() || statbuf.is_null() {
        if let Some(orig) = ORIGINAL_FUNCTIONS.xstat {
            return unsafe { orig(ver, pathname, statbuf) };
        }
        return -1;
    }

    let path_str = match unsafe { CStr::from_ptr(pathname).to_str() } {
        Ok(s) => s,
        Err(_) => {
            if let Some(orig_xstat) = ORIGINAL_FUNCTIONS.xstat {
                return unsafe { orig_xstat(ver, pathname, statbuf) };
            }
            return -1;
        }
    };

    if let Some(redirected) = PATH_REDIRECTOR.redirect(path_str) {
        debug!("__xstat: {} -> {}", path_str, redirected);
        let new_path = CString::new(redirected).unwrap();
        if let Some(orig_xstat) = ORIGINAL_FUNCTIONS.xstat {
            return unsafe { orig_xstat(ver, new_path.as_ptr(), statbuf) };
        }
    }

    if let Some(orig_xstat) = ORIGINAL_FUNCTIONS.xstat {
        return unsafe { orig_xstat(ver, pathname, statbuf) };
    }
    -1
}

/// Intercept __xstat64 (glibc wrapper for stat64)
#[unsafe(no_mangle)]
pub unsafe extern "C" fn __xstat64(
    ver: c_int,
    pathname: *const c_char,
    statbuf: *mut libc::stat64,
) -> c_int {
    if pathname.is_null() || statbuf.is_null() {
        if let Some(orig) = ORIGINAL_FUNCTIONS.xstat64 {
            return unsafe { orig(ver, pathname, statbuf) };
        }
        return -1;
    }

    let path_str = match unsafe { CStr::from_ptr(pathname).to_str() } {
        Ok(s) => s,
        Err(_) => {
            if let Some(orig_xstat64) = ORIGINAL_FUNCTIONS.xstat64 {
                return unsafe { orig_xstat64(ver, pathname, statbuf) };
            }
            return -1;
        }
    };

    if let Some(redirected) = PATH_REDIRECTOR.redirect(path_str) {
        debug!("__xstat64: {} -> {}", path_str, redirected);
        let new_path = CString::new(redirected).unwrap();
        if let Some(orig_xstat64) = ORIGINAL_FUNCTIONS.xstat64 {
            return unsafe { orig_xstat64(ver, new_path.as_ptr(), statbuf) };
        }
    }

    if let Some(orig_xstat64) = ORIGINAL_FUNCTIONS.xstat64 {
        return unsafe { orig_xstat64(ver, pathname, statbuf) };
    }
    -1
}

/// Intercept __lxstat (glibc wrapper for lstat)
#[unsafe(no_mangle)]
pub unsafe extern "C" fn __lxstat(
    ver: c_int,
    pathname: *const c_char,
    statbuf: *mut libc::stat,
) -> c_int {
    if pathname.is_null() || statbuf.is_null() {
        if let Some(orig) = ORIGINAL_FUNCTIONS.lxstat {
            return unsafe { orig(ver, pathname, statbuf) };
        }
        return -1;
    }

    let path_str = match unsafe { CStr::from_ptr(pathname).to_str() } {
        Ok(s) => s,
        Err(_) => {
            if let Some(orig_lxstat) = ORIGINAL_FUNCTIONS.lxstat {
                return unsafe { orig_lxstat(ver, pathname, statbuf) };
            }
            return -1;
        }
    };

    if let Some(redirected) = PATH_REDIRECTOR.redirect(path_str) {
        debug!("__lxstat: {} -> {}", path_str, redirected);
        let new_path = CString::new(redirected).unwrap();
        if let Some(orig_lxstat) = ORIGINAL_FUNCTIONS.lxstat {
            return unsafe { orig_lxstat(ver, new_path.as_ptr(), statbuf) };
        }
    }

    if let Some(orig_lxstat) = ORIGINAL_FUNCTIONS.lxstat {
        return unsafe { orig_lxstat(ver, pathname, statbuf) };
    }
    -1
}

/// Intercept __lxstat64 (glibc wrapper for lstat64)
#[unsafe(no_mangle)]
pub unsafe extern "C" fn __lxstat64(
    ver: c_int,
    pathname: *const c_char,
    statbuf: *mut libc::stat64,
) -> c_int {
    if pathname.is_null() || statbuf.is_null() {
        if let Some(orig) = ORIGINAL_FUNCTIONS.lxstat64 {
            return unsafe { orig(ver, pathname, statbuf) };
        }
        return -1;
    }

    let path_str = match unsafe { CStr::from_ptr(pathname).to_str() } {
        Ok(s) => s,
        Err(_) => {
            if let Some(orig_lxstat64) = ORIGINAL_FUNCTIONS.lxstat64 {
                return unsafe { orig_lxstat64(ver, pathname, statbuf) };
            }
            return -1;
        }
    };

    if let Some(redirected) = PATH_REDIRECTOR.redirect(path_str) {
        debug!("__lxstat64: {} -> {}", path_str, redirected);
        let new_path = CString::new(redirected).unwrap();
        if let Some(orig_lxstat64) = ORIGINAL_FUNCTIONS.lxstat64 {
            return unsafe { orig_lxstat64(ver, new_path.as_ptr(), statbuf) };
        }
    }

    if let Some(orig_lxstat64) = ORIGINAL_FUNCTIONS.lxstat64 {
        return unsafe { orig_lxstat64(ver, pathname, statbuf) };
    }
    -1
}

/// Intercept poll() to monitor udev fds
#[unsafe(no_mangle)]
pub unsafe extern "C" fn poll(fds: *mut libc::pollfd, nfds: libc::nfds_t, timeout: c_int) -> c_int {
    // Check if any udev fds are being polled
    if !fds.is_null() && nfds > 0 {
        let fds_slice = unsafe { std::slice::from_raw_parts(fds, nfds as usize) };
        for pfd in fds_slice {
            if syscalls::is_uinput_fd(pfd.fd) {
                tracing::trace!("poll: uinput fd {} being polled", pfd.fd);
            }
            if syscalls::is_udev_monitor_fd(pfd.fd) {
                tracing::trace!("poll: UDEV MONITOR fd {} (events={:x})", pfd.fd, pfd.events);
            }
        }
    }

    if let Some(orig_poll) = ORIGINAL_FUNCTIONS.poll {
        return unsafe { orig_poll(fds, nfds, timeout) };
    }
    -1
}

// Intercept epoll_wait
#[unsafe(no_mangle)]
pub unsafe extern "C" fn epoll_wait(
    epfd: c_int,
    events: *mut libc::epoll_event,
    maxevents: c_int,
    timeout: c_int,
) -> c_int {
    if let Some(orig_epoll_wait) = ORIGINAL_FUNCTIONS.epoll_wait {
        let result = unsafe { orig_epoll_wait(epfd, events, maxevents, timeout) };

        // Log which fds got events
        if result > 0 && !events.is_null() {
            let events_slice = unsafe { std::slice::from_raw_parts(events, result as usize) };
            for event in events_slice {
                let fd = event.u64 as c_int; // Typically fd stored in u64
                if syscalls::is_uinput_fd(fd) {
                    tracing::trace!("epoll_wait: uinput fd {} ready (events={:x})", fd, {
                        event.events
                    });
                }
                if syscalls::is_udev_monitor_fd(fd) {
                    tracing::trace!("epoll_wait: UDEV MONITOR fd {} ready (events={:x})", fd, {
                        event.events
                    });
                }
            }
        }

        return result;
    }
    -1
}

// Intercept epoll_pwait
#[unsafe(no_mangle)]
pub unsafe extern "C" fn epoll_pwait(
    epfd: c_int,
    events: *mut libc::epoll_event,
    maxevents: c_int,
    timeout: c_int,
    sigmask: *const libc::sigset_t,
) -> c_int {
    if let Some(orig_epoll_pwait) = ORIGINAL_FUNCTIONS.epoll_pwait {
        let result = unsafe { orig_epoll_pwait(epfd, events, maxevents, timeout, sigmask) };

        // Log which fds got events
        if result > 0 && !events.is_null() {
            let events_slice = unsafe { std::slice::from_raw_parts(events, result as usize) };
            for event in events_slice {
                let fd = event.u64 as c_int; // Typically fd stored in u64
                if syscalls::is_uinput_fd(fd) {
                    tracing::trace!("epoll_pwait: uinput fd {} ready (events={:x})", fd, {
                        event.events
                    });
                }
                if syscalls::is_udev_monitor_fd(fd) {
                    tracing::trace!("epoll_pwait: UDEV MONITOR fd {} ready (events={:x})", fd, {
                        event.events
                    });
                }
            }
        }

        return result;
    }
    -1
}

// Intercept inotify_init
#[unsafe(no_mangle)]
pub unsafe extern "C" fn inotify_init() -> c_int {
    if let Some(orig) = ORIGINAL_FUNCTIONS.inotify_init {
        let result = unsafe { orig() };
        tracing::trace!("inotify_init: fd={}", result);
        return result;
    }
    -1
}

// Intercept inotify_init1
#[unsafe(no_mangle)]
pub unsafe extern "C" fn inotify_init1(flags: c_int) -> c_int {
    if let Some(orig) = ORIGINAL_FUNCTIONS.inotify_init1 {
        let result = unsafe { orig(flags) };
        tracing::trace!("inotify_init1: fd={}", result);
        return result;
    }
    -1
}

// Intercept inotify_add_watch
#[unsafe(no_mangle)]
pub unsafe extern "C" fn inotify_add_watch(fd: c_int, pathname: *const c_char, mask: u32) -> c_int {
    if pathname.is_null() {
        if let Some(orig) = ORIGINAL_FUNCTIONS.inotify_add_watch {
            return unsafe { orig(fd, pathname, mask) };
        }
        return -1;
    }

    let path_str = unsafe { CStr::from_ptr(pathname).to_str().unwrap_or("") };

    tracing::trace!(
        "inotify_add_watch called for path: {}, mask={:#x}",
        path_str,
        mask
    );

    // Redirect /dev/input to our fake directory!
    if path_str == "/dev/input" || path_str.starts_with("/dev/input/") {
        let redirected = PATH_REDIRECTOR
            .redirect(path_str)
            .unwrap_or_else(|| format!("{}/devices", syscalls::get_base_path()));

        tracing::trace!(
            "inotify_add_watch redirected: {} -> {}",
            path_str,
            redirected
        );

        let new_path = CString::new(redirected).unwrap();
        if let Some(orig) = ORIGINAL_FUNCTIONS.inotify_add_watch {
            let result = unsafe { orig(fd, new_path.as_ptr(), mask) };
            tracing::trace!("inotify_add_watch result: {}", result);
            return result;
        }
        return -1;
    }

    if let Some(orig) = ORIGINAL_FUNCTIONS.inotify_add_watch {
        return unsafe { orig(fd, pathname, mask) };
    }
    -1
}

/// Intercept socket() to track Unix domain sockets
#[unsafe(no_mangle)]
pub unsafe extern "C" fn socket(domain: c_int, type_: c_int, protocol: c_int) -> c_int {
    if let Some(orig_socket) = ORIGINAL_FUNCTIONS.socket {
        let fd = unsafe { orig_socket(domain, type_, protocol) };

        // Track Unix domain sockets (AF_UNIX = 1)
        if fd >= 0 && domain == 1 {
            syscalls::track_unix_socket(fd);
        }

        return fd;
    }
    -1
}

/// Intercept connect() to redirect /run/udev/control
#[unsafe(no_mangle)]
pub unsafe extern "C" fn connect(
    sockfd: c_int,
    addr: *const libc::sockaddr,
    addrlen: libc::socklen_t,
) -> c_int {
    // Check if this is a Unix domain socket we're tracking
    if syscalls::is_tracked_unix_socket(sockfd) && !addr.is_null() {
        let sa_family = unsafe { (*addr).sa_family };
        if sa_family == 1 {
            // AF_UNIX
            let unix_addr = addr as *const libc::sockaddr_un;
            let path_bytes = unsafe { &(*unix_addr).sun_path };

            // Find null terminator
            let path_len = path_bytes.iter().position(|&b| b == 0).unwrap_or(108);
            let path_slice = &path_bytes[..path_len];
            let path_as_u8: Vec<u8> = path_slice.iter().map(|&c| c as u8).collect();

            if let Ok(path_str) = std::str::from_utf8(&path_as_u8) {
                if let Some(redirected) = PATH_REDIRECTOR.redirect(path_str) {
                    debug!("connect: {} -> {}", path_str, redirected);

                    // Create new sockaddr_un with redirected path
                    let mut new_addr: libc::sockaddr_un = unsafe { std::mem::zeroed() };
                    new_addr.sun_family = 1; // AF_UNIX

                    // Convert redirected path to c_char array
                    let new_path = CString::new(redirected).unwrap();
                    let new_path_bytes = new_path.as_bytes_with_nul();
                    for (i, &byte) in new_path_bytes.iter().enumerate() {
                        if i < new_addr.sun_path.len() {
                            new_addr.sun_path[i] = byte as c_char;
                        }
                    }

                    if let Some(orig_connect) = ORIGINAL_FUNCTIONS.connect {
                        let result = unsafe {
                            orig_connect(
                                sockfd,
                                &new_addr as *const _ as *const libc::sockaddr,
                                size_of::<libc::sockaddr_un>() as libc::socklen_t,
                            )
                        };

                        // Track this as a udev monitor fd if connection succeeded
                        if result == 0 && path_str == "/run/udev/control" {
                            syscalls::register_udev_monitor_fd(sockfd);
                        }

                        return result;
                    }
                }
            }
        }
    }

    if let Some(orig_connect) = ORIGINAL_FUNCTIONS.connect {
        return unsafe { orig_connect(sockfd, addr, addrlen) };
    }
    -1
}

/// Intercept bind() in case someone tries to create /run/udev/control
#[unsafe(no_mangle)]
pub unsafe extern "C" fn bind(
    sockfd: c_int,
    addr: *const libc::sockaddr,
    addrlen: libc::socklen_t,
) -> c_int {
    if let Some(orig_bind) = ORIGINAL_FUNCTIONS.bind {
        return unsafe { orig_bind(sockfd, addr, addrlen) };
    }
    -1
}
