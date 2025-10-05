use libc::{c_char, c_int, c_void};
use std::collections::HashMap;
use std::ffi::{CStr, CString};
use std::os::unix::io::AsRawFd;
use std::os::unix::io::RawFd;
use std::os::unix::net::UnixStream;
use std::ptr;
use std::sync::Mutex;
use tracing::{debug, info, warn};

lazy_static::lazy_static! {
    static ref FAKE_UDEV_CONTEXTS: Mutex<HashMap<usize, FakeUdevContext>> = Mutex::new(HashMap::new());
    static ref FAKE_UDEV_MONITORS: Mutex<HashMap<usize, FakeUdevMonitor>> = Mutex::new(HashMap::new());
    static ref FAKE_UDEV_ENUMERATES: Mutex<HashMap<usize, FakeUdevEnumerate>> = Mutex::new(HashMap::new());
    static ref NEXT_FAKE_PTR: Mutex<usize> = Mutex::new(0x1000); // Start at a safe offset
}

struct FakeUdevContext {
    ptr: usize,
}

struct FakeUdevMonitor {
    socket: Option<UnixStream>,
    fd: RawFd,
}

struct FakeUdevEnumerate {
    devices: Vec<String>,
}

/// Get the path to our fake udev socket
fn get_udev_socket_path() -> String {
    let uid = unsafe { libc::getuid() };
    let base_path =
        std::env::var("VIMPUTTI_PATH").unwrap_or_else(|_| format!("/run/user/{}/vimputti", uid));
    format!("{}/udev", base_path)
}

/// Get next fake pointer
fn next_ptr() -> usize {
    let mut next = NEXT_FAKE_PTR.lock().unwrap();
    let ptr = *next;
    *next += 1;
    ptr
}

/// Intercept udev_new() - create fake udev context
#[unsafe(no_mangle)]
pub unsafe extern "C" fn udev_new() -> *mut c_void {
    let ptr = next_ptr();

    let context = FakeUdevContext { ptr };
    FAKE_UDEV_CONTEXTS.lock().unwrap().insert(ptr, context);

    debug!("udev_new: returning fake context {:x}", ptr);
    ptr as *mut c_void
}

/// Intercept udev_ref() - increment reference (no-op for us)
#[unsafe(no_mangle)]
pub unsafe extern "C" fn udev_ref(udev: *mut c_void) -> *mut c_void {
    debug!("udev_ref");
    udev
}

/// Intercept udev_unref() - cleanup fake context
#[unsafe(no_mangle)]
pub unsafe extern "C" fn udev_unref(udev: *mut c_void) -> *mut c_void {
    let ptr = udev as usize;

    if FAKE_UDEV_CONTEXTS.lock().unwrap().remove(&ptr).is_some() {
        debug!("udev_unref: cleaned up context {:x}", ptr);
    }

    ptr::null_mut()
}

/// Intercept udev_monitor_new_from_netlink() - create monitor connected to our socket
#[unsafe(no_mangle)]
pub unsafe extern "C" fn udev_monitor_new_from_netlink(
    udev: *mut c_void,
    name: *const c_char,
) -> *mut c_void {
    let name_str = if name.is_null() {
        "udev"
    } else {
        CStr::from_ptr(name).to_str().unwrap_or("udev")
    };

    debug!("udev_monitor_new_from_netlink: name={}", name_str);

    let monitor_ptr = next_ptr();

    // Try to connect to our fake udev socket
    let socket_path = get_udev_socket_path();
    debug!("Connecting to fake udev socket: {}", socket_path);

    let socket = match UnixStream::connect(&socket_path) {
        Ok(stream) => {
            let fd = stream.as_raw_fd();
            info!("Connected to fake udev socket, fd={}", fd);
            Some(stream)
        }
        Err(e) => {
            warn!(
                "Failed to connect to fake udev socket (this is OK if not using udev monitoring): {}",
                e
            );
            None
        }
    };

    let fd = socket.as_ref().map(|s| s.as_raw_fd()).unwrap_or(-1);

    let monitor = FakeUdevMonitor { socket, fd };
    FAKE_UDEV_MONITORS
        .lock()
        .unwrap()
        .insert(monitor_ptr, monitor);

    debug!("Created fake udev monitor: {:x}", monitor_ptr);
    monitor_ptr as *mut c_void
}

/// Intercept udev_monitor_ref()
#[unsafe(no_mangle)]
pub unsafe extern "C" fn udev_monitor_ref(udev_monitor: *mut c_void) -> *mut c_void {
    debug!("udev_monitor_ref");
    udev_monitor
}

/// Intercept udev_monitor_filter_add_match_subsystem_devtype()
#[unsafe(no_mangle)]
pub unsafe extern "C" fn udev_monitor_filter_add_match_subsystem_devtype(
    udev_monitor: *mut c_void,
    subsystem: *const c_char,
    devtype: *const c_char,
) -> c_int {
    let subsystem_str = if subsystem.is_null() {
        "none"
    } else {
        CStr::from_ptr(subsystem).to_str().unwrap_or("unknown")
    };

    debug!(
        "udev_monitor_filter_add_match_subsystem_devtype: subsystem={}",
        subsystem_str
    );
    0
}

/// Intercept udev_monitor_filter_update()
#[unsafe(no_mangle)]
pub unsafe extern "C" fn udev_monitor_filter_update(udev_monitor: *mut c_void) -> c_int {
    debug!("udev_monitor_filter_update");
    0
}

/// Intercept udev_monitor_enable_receiving()
#[unsafe(no_mangle)]
pub unsafe extern "C" fn udev_monitor_enable_receiving(udev_monitor: *mut c_void) -> c_int {
    debug!("udev_monitor_enable_receiving");
    0
}

/// Intercept udev_monitor_set_receive_buffer_size()
#[unsafe(no_mangle)]
pub unsafe extern "C" fn udev_monitor_set_receive_buffer_size(
    udev_monitor: *mut c_void,
    size: c_int,
) -> c_int {
    debug!("udev_monitor_set_receive_buffer_size: size={}", size);
    0
}

/// Intercept udev_monitor_get_fd() - return the socket FD
#[unsafe(no_mangle)]
pub unsafe extern "C" fn udev_monitor_get_fd(udev_monitor: *mut c_void) -> c_int {
    let monitor_ptr = udev_monitor as usize;

    let monitors = FAKE_UDEV_MONITORS.lock().unwrap();
    if let Some(monitor) = monitors.get(&monitor_ptr) {
        debug!("udev_monitor_get_fd: returning fd={}", monitor.fd);
        return monitor.fd;
    }

    warn!("udev_monitor_get_fd: unknown monitor {:x}", monitor_ptr);
    -1
}

/// Intercept udev_monitor_receive_device() - read device event from our socket
#[unsafe(no_mangle)]
pub unsafe extern "C" fn udev_monitor_receive_device(udev_monitor: *mut c_void) -> *mut c_void {
    debug!("udev_monitor_receive_device");

    // For now, return null (no device available)
    // Steam will poll the FD and call this when data is available
    // TODO: Parse udev messages from socket and create fake device objects
    ptr::null_mut()
}

/// Intercept udev_monitor_unref()
#[unsafe(no_mangle)]
pub unsafe extern "C" fn udev_monitor_unref(udev_monitor: *mut c_void) -> *mut c_void {
    let monitor_ptr = udev_monitor as usize;

    if FAKE_UDEV_MONITORS
        .lock()
        .unwrap()
        .remove(&monitor_ptr)
        .is_some()
    {
        debug!("udev_monitor_unref: cleaned up monitor {:x}", monitor_ptr);
    }

    ptr::null_mut()
}

/// Intercept udev_enumerate_new() - create device enumeration
#[unsafe(no_mangle)]
pub unsafe extern "C" fn udev_enumerate_new(udev: *mut c_void) -> *mut c_void {
    let enum_ptr = next_ptr();

    let enumerate = FakeUdevEnumerate {
        devices: Vec::new(),
    };

    FAKE_UDEV_ENUMERATES
        .lock()
        .unwrap()
        .insert(enum_ptr, enumerate);

    debug!("udev_enumerate_new: returning {:x}", enum_ptr);
    enum_ptr as *mut c_void
}

/// Intercept udev_enumerate_ref()
#[unsafe(no_mangle)]
pub unsafe extern "C" fn udev_enumerate_ref(udev_enumerate: *mut c_void) -> *mut c_void {
    debug!("udev_enumerate_ref");
    udev_enumerate
}

/// Intercept udev_enumerate_add_match_subsystem()
#[unsafe(no_mangle)]
pub unsafe extern "C" fn udev_enumerate_add_match_subsystem(
    udev_enumerate: *mut c_void,
    subsystem: *const c_char,
) -> c_int {
    let subsystem_str = if subsystem.is_null() {
        "none"
    } else {
        CStr::from_ptr(subsystem).to_str().unwrap_or("unknown")
    };

    debug!(
        "udev_enumerate_add_match_subsystem: subsystem={}",
        subsystem_str
    );
    0
}

/// Intercept udev_enumerate_add_match_property()
#[unsafe(no_mangle)]
pub unsafe extern "C" fn udev_enumerate_add_match_property(
    udev_enumerate: *mut c_void,
    property: *const c_char,
    value: *const c_char,
) -> c_int {
    debug!("udev_enumerate_add_match_property");
    0
}

/// Intercept udev_enumerate_scan_devices()
#[unsafe(no_mangle)]
pub unsafe extern "C" fn udev_enumerate_scan_devices(udev_enumerate: *mut c_void) -> c_int {
    debug!("udev_enumerate_scan_devices");

    // TODO: Populate with our virtual devices from /run/user/X/vimputti/devices
    // For now, just return success with empty list
    0
}

/// Intercept udev_enumerate_get_list_entry()
#[unsafe(no_mangle)]
pub unsafe extern "C" fn udev_enumerate_get_list_entry(udev_enumerate: *mut c_void) -> *mut c_void {
    debug!("udev_enumerate_get_list_entry: returning empty list");

    // Return null = empty list
    // Steam should be OK with this and will just see no devices via udev
    // (but will still find them via /dev/input scandir)
    ptr::null_mut()
}

/// Intercept udev_enumerate_unref()
#[unsafe(no_mangle)]
pub unsafe extern "C" fn udev_enumerate_unref(udev_enumerate: *mut c_void) -> *mut c_void {
    let enum_ptr = udev_enumerate as usize;

    if FAKE_UDEV_ENUMERATES
        .lock()
        .unwrap()
        .remove(&enum_ptr)
        .is_some()
    {
        debug!("udev_enumerate_unref: cleaned up enumerate {:x}", enum_ptr);
    }

    ptr::null_mut()
}

/// Intercept udev_device_get_syspath()
#[unsafe(no_mangle)]
pub unsafe extern "C" fn udev_device_get_syspath(udev_device: *mut c_void) -> *const c_char {
    debug!("udev_device_get_syspath: returning null");
    ptr::null()
}

/// Intercept udev_device_unref()
#[unsafe(no_mangle)]
pub unsafe extern "C" fn udev_device_unref(udev_device: *mut c_void) -> *mut c_void {
    debug!("udev_device_unref");
    ptr::null_mut()
}
