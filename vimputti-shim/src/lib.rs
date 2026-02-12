//! Minimal vimputti shim
//!
//! With real mknod() device nodes, we NO LONGER NEED:
//! - Path redirection
//! - Device emulation
//! - Fake sysfs/udev
//!
//! This shim now ONLY intercepts ioctl() to track/log uinput device creation,
//! in case apps try to create their own uinput devices.

#![feature(c_variadic)]

use lazy_static::lazy_static;
use libc::{c_int, c_long, c_void};

lazy_static! {
    static ref ORIGINAL_IOCTL: Option<unsafe extern "C" fn(c_int, c_long, ...) -> c_int> = {
        unsafe {
            let name_cstr = std::ffi::CString::new("ioctl").ok()?;
            let ptr = libc::dlsym(libc::RTLD_NEXT, name_cstr.as_ptr());
            if ptr.is_null() {
                None
            } else {
                Some(std::mem::transmute_copy(&ptr))
            }
        }
    };
}

// Initialize tracing when shim loads
#[ctor::ctor]
fn init_shim() {
    tracing_subscriber::fmt()
        .with_env_filter(
            tracing_subscriber::EnvFilter::try_from_default_env()
                .unwrap_or_else(|_| tracing_subscriber::EnvFilter::new("info")),
        )
        .init();
}

// uinput ioctl constants
const UI_DEV_CREATE: c_long = 0x5501;
const UI_DEV_DESTROY: c_long = 0x5502;
const UI_GET_SYSNAME: c_long = 0x8000552c_u32 as c_long; // _IOR('U', 44, 80)

/// Intercept ioctl() - only for logging/tracking uinput device creation
#[unsafe(no_mangle)]
pub unsafe extern "C" fn ioctl(fd: c_int, request: c_long, mut args: ...) -> c_int {
    // Log uinput device creation/destruction
    match request {
        UI_DEV_CREATE => {
            // Check if this FD is actually /dev/uinput
            if is_uinput_fd(fd) {
                tracing::info!("App created uinput device on fd {}", fd);
            }
        }
        UI_DEV_DESTROY => {
            if is_uinput_fd(fd) {
                tracing::info!("App destroyed uinput device on fd {}", fd);
            }
        }
        UI_GET_SYSNAME => {
            if is_uinput_fd(fd) {
                tracing::debug!("App queried uinput sysname on fd {}", fd);
            }
        }
        _ => {}
    }

    // Always pass through to real ioctl
    if let Some(orig_ioctl) = *ORIGINAL_IOCTL {
        let arg: *mut c_void = unsafe { args.arg() };
        return unsafe { orig_ioctl(fd, request, arg) };
    }

    -1
}

// Helper to check if FD points to /dev/uinput
fn is_uinput_fd(fd: c_int) -> bool {
    let fd_path = format!("/proc/self/fd/{}", fd);
    if let Ok(target) = std::fs::read_link(&fd_path) {
        if let Some(target_str) = target.to_str() {
            return target_str.contains("/dev/uinput");
        }
    }
    false
}
