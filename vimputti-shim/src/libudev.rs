use libc::{c_char, c_int, c_void};
use std::collections::HashMap;
use std::ffi::{CStr, CString};
use std::os::unix::io::AsRawFd;
use std::os::unix::io::RawFd;
use std::os::unix::net::UnixStream;
use std::ptr;
use std::sync::Mutex;
use tracing::{debug, trace};

lazy_static::lazy_static! {
    static ref FAKE_UDEV_CONTEXTS: Mutex<HashMap<usize, FakeUdevContext>> = Mutex::new(HashMap::new());
    static ref FAKE_UDEV_MONITORS: Mutex<HashMap<usize, FakeUdevMonitor>> = Mutex::new(HashMap::new());
    static ref FAKE_UDEV_ENUMERATES: Mutex<HashMap<usize, FakeUdevEnumerate>> = Mutex::new(HashMap::new());
    static ref FAKE_UDEV_DEVICES: Mutex<HashMap<usize, FakeUdevDevice>> = Mutex::new(HashMap::new());
    static ref FAKE_UDEV_LIST_ENTRIES: Mutex<HashMap<usize, FakeUdevListEntry>> = Mutex::new(HashMap::new());
    static ref NEXT_FAKE_PTR: Mutex<usize> = Mutex::new(0x1000);
    static ref STRING_CACHE: Mutex<Vec<CString>> = Mutex::new(Vec::new());
}

struct FakeUdevEnumerate {
    devices: Vec<FakeUdevDevice>,
    current_entry: Option<usize>,
}

#[derive(Clone)]
struct FakeUdevDevice {
    syspath: String,
    devnode: String,
    subsystem: String,
    properties: HashMap<String, String>,
}

struct FakeUdevListEntry {
    enum_ptr: usize,
    index: usize,
}

struct FakeUdevContext {
    ptr: usize,
}

struct FakeUdevMonitor {
    socket: Option<UnixStream>,
    fd: RawFd,
}

/// Helper to create a cached CString pointer
fn cache_cstring(s: String) -> *const c_char {
    let cstr = CString::new(s).unwrap();
    let ptr = cstr.as_ptr();
    STRING_CACHE.lock().unwrap().push(cstr);
    ptr
}

/// Create a fake udev device from a DeviceConfig
fn create_fake_device_from_config(
    devnode: String,
    config: &vimputti::DeviceConfig,
) -> FakeUdevDevice {
    let filename = std::path::Path::new(&devnode)
        .file_name()
        .and_then(|n| n.to_str())
        .unwrap_or("unknown");

    let base_path = crate::syscalls::get_base_path();
    let syspath = format!("{}/sys/devices/virtual/input/{}", base_path, filename);

    let mut properties = HashMap::new();

    // Use config directly - no file I/O needed!
    if filename.starts_with("event") || filename.starts_with("js") {
        properties.insert("ID_INPUT".to_string(), "1".to_string());
        properties.insert("ID_INPUT_JOYSTICK".to_string(), "1".to_string());
        properties.insert("SUBSYSTEM".to_string(), "input".to_string());
    }

    // Vendor/Product info from config
    properties.insert(
        "ID_VENDOR_ID".to_string(),
        format!("{:04x}", config.vendor_id),
    );
    properties.insert(
        "ID_MODEL_ID".to_string(),
        format!("{:04x}", config.product_id),
    );
    properties.insert("ID_VENDOR".to_string(), format!("{:04x}", config.vendor_id));
    properties.insert("ID_MODEL".to_string(), format!("{:04x}", config.product_id));

    // Vendor name from config
    let vendor_name = match config.vendor_id {
        0x045e => "Microsoft",
        0x054c => "Sony",
        0x057e => "Nintendo",
        _ => "Unknown",
    };

    properties.insert("ID_VENDOR_ENC".to_string(), vendor_name.to_string());
    properties.insert(
        "ID_VENDOR_FROM_DATABASE".to_string(),
        vendor_name.to_string(),
    );
    properties.insert(
        "ID_MODEL_ENC".to_string(),
        config.name.replace(' ', "\\x20"),
    );
    properties.insert("ID_MODEL_FROM_DATABASE".to_string(), config.name.clone());
    properties.insert("ID_PRODUCT_FROM_DATABASE".to_string(), config.name.clone());

    // Bus type from config
    let bus_name = match config.bustype {
        vimputti::BusType::Usb => "usb",
        vimputti::BusType::Bluetooth => "bluetooth",
        vimputti::BusType::Virtual => "virtual",
    };
    properties.insert("ID_BUS".to_string(), bus_name.to_string());

    if matches!(config.bustype, vimputti::BusType::Usb) {
        properties.insert("ID_USB_INTERFACES".to_string(), ":030000:".to_string());
        properties.insert("ID_USB_INTERFACE_NUM".to_string(), "00".to_string());
    }

    // Other properties
    properties.insert(
        "ID_PATH".to_string(),
        format!("platform-vimputti-{}", filename),
    );
    properties.insert(
        "ID_PATH_TAG".to_string(),
        format!("platform-vimputti-{}", filename),
    );
    properties.insert("ID_SERIAL".to_string(), format!("vimputti_{}", filename));
    properties.insert("DEVNAME".to_string(), devnode.clone());
    properties.insert(
        "DEVPATH".to_string(),
        format!("/devices/virtual/input/{}", filename),
    );
    properties.insert("MAJOR".to_string(), "13".to_string());
    properties.insert(
        "MINOR".to_string(),
        if filename.starts_with("event") {
            "64"
        } else {
            "0"
        }
        .to_string(),
    );
    properties.insert("TAGS".to_string(), ":uaccess:".to_string());

    debug!(
        "Created fake device from config: {} (vendor={:04x} product={:04x})",
        config.name, config.vendor_id, config.product_id
    );

    FakeUdevDevice {
        syspath,
        devnode,
        subsystem: "input".to_string(),
        properties,
    }
}

/// Get list of virtual device paths WITH their configs
fn get_virtual_devices_with_configs() -> Vec<(String, vimputti::DeviceConfig)> {
    let base_path = crate::syscalls::get_base_path();
    let devices_dir = std::path::Path::new(&base_path).join("devices");

    // Get configs from currently open devices
    let device_configs = crate::syscalls::get_all_device_configs();

    // Also scan directory for any unopened devices
    let mut devices = Vec::new();

    // First, add opened devices (we have full config for these)
    for (event_node, config) in device_configs {
        let devnode = devices_dir.join(&event_node).to_string_lossy().to_string();
        devices.push((devnode, config));
    }

    // TODO: For unopened devices, we could read from sysfs as fallback,
    // but Steam will typically open devices before enumerating, so this is rare

    debug!("Found {} virtual devices with configs", devices.len());
    devices
}

/// Get the path to our fake udev socket
fn get_udev_socket_path() -> String {
    "/tmp/vimputti/udev".to_string()
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

    debug!("[UDEV] udev_new: {:x}", ptr);
    ptr as *mut c_void
}

/// Intercept udev_ref() - increment reference (no-op for us)
#[unsafe(no_mangle)]
pub unsafe extern "C" fn udev_ref(udev: *mut c_void) -> *mut c_void {
    udev
}

/// Intercept udev_unref() - cleanup fake context
#[unsafe(no_mangle)]
pub unsafe extern "C" fn udev_unref(udev: *mut c_void) -> *mut c_void {
    let ptr = udev as usize;
    FAKE_UDEV_CONTEXTS.lock().unwrap().remove(&ptr);
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
        unsafe { CStr::from_ptr(name).to_str().unwrap_or("udev") }
    };

    trace!("[UDEV] udev_monitor_new_from_netlink: name={}", name_str);

    let monitor_ptr = next_ptr();

    let socket_path = get_udev_socket_path();
    debug!("[UDEV] Connecting to fake udev socket at {}", socket_path);

    let socket = match UnixStream::connect(&socket_path) {
        Ok(stream) => {
            if let Err(e) = stream.set_nonblocking(true) {
                debug!("[UDEV] Failed to set non-blocking: {}", e);
            }

            let fd = stream.as_raw_fd();
            crate::syscalls::register_udev_monitor_fd(fd);
            debug!(
                "[UDEV] *** CONNECTED: fd={}, monitor_ptr={:x} ***",
                fd, monitor_ptr
            );
            Some(stream)
        }
        Err(e) => {
            debug!("[UDEV] Failed to connect: {}", e);
            None
        }
    };

    let fd = socket.as_ref().map(|s| s.as_raw_fd()).unwrap_or(-1);

    let monitor = FakeUdevMonitor { socket, fd };
    FAKE_UDEV_MONITORS
        .lock()
        .unwrap()
        .insert(monitor_ptr, monitor);

    debug!("[UDEV] Created monitor {:x} with fd={}", monitor_ptr, fd);
    monitor_ptr as *mut c_void
}

/// Intercept udev_monitor_ref()
#[unsafe(no_mangle)]
pub unsafe extern "C" fn udev_monitor_ref(udev_monitor: *mut c_void) -> *mut c_void {
    udev_monitor
}

/// Intercept udev_monitor_filter_add_match_subsystem_devtype()
#[unsafe(no_mangle)]
pub unsafe extern "C" fn udev_monitor_filter_add_match_subsystem_devtype(
    udev_monitor: *mut c_void,
    subsystem: *const c_char,
    devtype: *const c_char,
) -> c_int {
    let monitor_ptr = udev_monitor as usize;
    let subsystem_str = if subsystem.is_null() {
        "none"
    } else {
        unsafe { CStr::from_ptr(subsystem).to_str().unwrap_or("unknown") }
    };

    trace!(
        "[UDEV] filter_add_match for monitor {:x}: subsystem={}",
        monitor_ptr, subsystem_str
    );
    0
}

/// Intercept udev_monitor_filter_update()
#[unsafe(no_mangle)]
pub unsafe extern "C" fn udev_monitor_filter_update(udev_monitor: *mut c_void) -> c_int {
    0
}

/// Intercept udev_monitor_enable_receiving()
#[unsafe(no_mangle)]
pub unsafe extern "C" fn udev_monitor_enable_receiving(udev_monitor: *mut c_void) -> c_int {
    let monitor_ptr = udev_monitor as usize;
    trace!(
        "[UDEV] udev_monitor_enable_receiving called for {:x}",
        monitor_ptr
    );
    0
}

/// Intercept udev_monitor_set_receive_buffer_size()
#[unsafe(no_mangle)]
pub unsafe extern "C" fn udev_monitor_set_receive_buffer_size(
    udev_monitor: *mut c_void,
    size: c_int,
) -> c_int {
    0
}

/// Intercept udev_monitor_get_fd() - return the socket FD
#[unsafe(no_mangle)]
pub unsafe extern "C" fn udev_monitor_get_fd(udev_monitor: *mut c_void) -> c_int {
    let monitor_ptr = udev_monitor as usize;

    let monitors = FAKE_UDEV_MONITORS.lock().unwrap();
    if let Some(monitor) = monitors.get(&monitor_ptr) {
        trace!(
            "!!! [UDEV] udev_monitor_get_fd: returning fd={} for monitor {:x} !!!",
            monitor.fd, monitor_ptr
        );
        return monitor.fd;
    }
    trace!(
        "!!! [UDEV] udev_monitor_get_fd: monitor {:x} NOT FOUND !!!",
        monitor_ptr
    );
    -1
}

/// Intercept udev_monitor_receive_device() - read device event from our socket
#[unsafe(no_mangle)]
pub unsafe extern "C" fn udev_monitor_receive_device(udev_monitor: *mut c_void) -> *mut c_void {
    use std::io::Read;

    let monitor_ptr = udev_monitor as usize;

    trace!(
        "[UDEV] udev_monitor_receive_device called for {:x}",
        monitor_ptr
    );

    let mut monitors = FAKE_UDEV_MONITORS.lock().unwrap();
    if let Some(monitor) = monitors.get_mut(&monitor_ptr) {
        if let Some(socket) = &mut monitor.socket {
            // Read message from socket
            let mut buffer = vec![0u8; 4096];

            match socket.read(&mut buffer) {
                Ok(0) => {
                    debug!("[UDEV] Socket closed");
                    return ptr::null_mut();
                }
                Ok(n) => {
                    let message = String::from_utf8_lossy(&buffer[..n]);
                    debug!(
                        "[UDEV] Received {} bytes: {}",
                        n,
                        message.lines().next().unwrap_or("")
                    );

                    // Parse the message
                    let device = parse_udev_message(&message);

                    if let Some(device) = device {
                        let device_ptr = next_ptr();
                        FAKE_UDEV_DEVICES.lock().unwrap().insert(device_ptr, device);
                        debug!("[UDEV] Created device from monitor event: {:x}", device_ptr);
                        return device_ptr as *mut c_void;
                    }
                }
                Err(e) if e.kind() == std::io::ErrorKind::WouldBlock => {
                    // No data available right now
                    return ptr::null_mut();
                }
                Err(e) => {
                    debug!("[UDEV] Socket read error: {}", e);
                    return ptr::null_mut();
                }
            }
        }
    }
    ptr::null_mut()
}

/// Intercept udev_monitor_unref()
#[unsafe(no_mangle)]
pub unsafe extern "C" fn udev_monitor_unref(udev_monitor: *mut c_void) -> *mut c_void {
    let monitor_ptr = udev_monitor as usize;
    trace!("[UDEV] udev_monitor_unref called for {:x}", monitor_ptr);
    FAKE_UDEV_MONITORS.lock().unwrap().remove(&monitor_ptr);
    ptr::null_mut()
}

/// Intercept udev_enumerate_new() - create device enumeration
#[unsafe(no_mangle)]
pub unsafe extern "C" fn udev_enumerate_new(udev: *mut c_void) -> *mut c_void {
    let enum_ptr = next_ptr();

    let enumerate = FakeUdevEnumerate {
        devices: Vec::new(),
        current_entry: None,
    };

    FAKE_UDEV_ENUMERATES
        .lock()
        .unwrap()
        .insert(enum_ptr, enumerate);

    enum_ptr as *mut c_void
}

/// Intercept udev_enumerate_ref()
#[unsafe(no_mangle)]
pub unsafe extern "C" fn udev_enumerate_ref(udev_enumerate: *mut c_void) -> *mut c_void {
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
        unsafe { CStr::from_ptr(subsystem).to_str().unwrap_or("unknown") }
    };

    debug!(
        "[UDEV] udev_enumerate_add_match_subsystem: subsystem={}",
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
    0
}

/// Intercept udev_enumerate_unref()
#[unsafe(no_mangle)]
pub unsafe extern "C" fn udev_enumerate_unref(udev_enumerate: *mut c_void) -> *mut c_void {
    let enum_ptr = udev_enumerate as usize;
    FAKE_UDEV_ENUMERATES.lock().unwrap().remove(&enum_ptr);
    ptr::null_mut()
}

/// Intercept udev_device_get_syspath()
#[unsafe(no_mangle)]
pub unsafe extern "C" fn udev_device_get_syspath(udev_device: *mut c_void) -> *const c_char {
    ptr::null()
}

/// Intercept udev_device_unref()
#[unsafe(no_mangle)]
pub unsafe extern "C" fn udev_device_unref(udev_device: *mut c_void) -> *mut c_void {
    ptr::null_mut()
}

/// Intercept udev_enumerate_scan_devices()
#[unsafe(no_mangle)]
pub unsafe extern "C" fn udev_enumerate_scan_devices(udev_enumerate: *mut c_void) -> c_int {
    let enum_ptr = udev_enumerate as usize;
    debug!("[UDEV] udev_enumerate_scan_devices called");

    // Get virtual devices with their configs
    let device_list = get_virtual_devices_with_configs();

    // Create fake devices
    let devices: Vec<FakeUdevDevice> = device_list
        .into_iter()
        .map(|(devnode, config)| create_fake_device_from_config(devnode, &config))
        .collect();

    debug!(
        "[UDEV] udev_enumerate_scan_devices: found {} devices",
        devices.len()
    );

    // Update the enumerate with devices
    if let Some(enumerate) = FAKE_UDEV_ENUMERATES.lock().unwrap().get_mut(&enum_ptr) {
        enumerate.devices = devices;
        enumerate.current_entry = None;
    }
    0
}

/// Intercept udev_enumerate_get_list_entry()
#[unsafe(no_mangle)]
pub unsafe extern "C" fn udev_enumerate_get_list_entry(udev_enumerate: *mut c_void) -> *mut c_void {
    let enum_ptr = udev_enumerate as usize;

    let enumerates = FAKE_UDEV_ENUMERATES.lock().unwrap();
    if let Some(enumerate) = enumerates.get(&enum_ptr) {
        if enumerate.devices.is_empty() {
            debug!("[UDEV] udev_enumerate_get_list_entry: no devices found");
            return ptr::null_mut();
        }

        drop(enumerates); // Release lock before creating new entry

        // Create first list entry
        let entry_ptr = next_ptr();
        let entry = FakeUdevListEntry { enum_ptr, index: 0 };

        FAKE_UDEV_LIST_ENTRIES
            .lock()
            .unwrap()
            .insert(entry_ptr, entry);

        debug!(
            "[UDEV] udev_enumerate_get_list_entry: returning entry {:x} (index 0)",
            entry_ptr
        );
        return entry_ptr as *mut c_void;
    }
    ptr::null_mut()
}

/// Intercept udev_list_entry_get_next()
#[unsafe(no_mangle)]
pub unsafe extern "C" fn udev_list_entry_get_next(list_entry: *mut c_void) -> *mut c_void {
    let entry_ptr = list_entry as usize;

    let entries = FAKE_UDEV_LIST_ENTRIES.lock().unwrap();
    if let Some(entry) = entries.get(&entry_ptr) {
        let enum_ptr = entry.enum_ptr;
        let next_index = entry.index + 1;

        drop(entries);

        // Check if there's a next device
        let enumerates = FAKE_UDEV_ENUMERATES.lock().unwrap();
        if let Some(enumerate) = enumerates.get(&enum_ptr) {
            if next_index >= enumerate.devices.len() {
                debug!("[UDEV] udev_list_entry_get_next: no more entries");
                return ptr::null_mut();
            }

            drop(enumerates);

            // Create next entry
            let next_entry_ptr = next_ptr();
            let next_entry = FakeUdevListEntry {
                enum_ptr,
                index: next_index,
            };

            FAKE_UDEV_LIST_ENTRIES
                .lock()
                .unwrap()
                .insert(next_entry_ptr, next_entry);

            debug!(
                "[UDEV] udev_list_entry_get_next: returning entry {:x} (index {})",
                next_entry_ptr, next_index
            );
            return next_entry_ptr as *mut c_void;
        }
    }
    ptr::null_mut()
}

/// Intercept udev_list_entry_get_name()
#[unsafe(no_mangle)]
pub unsafe extern "C" fn udev_list_entry_get_name(list_entry: *mut c_void) -> *const c_char {
    let entry_ptr = list_entry as usize;

    let entries = FAKE_UDEV_LIST_ENTRIES.lock().unwrap();
    if let Some(entry) = entries.get(&entry_ptr) {
        let enum_ptr = entry.enum_ptr;
        let index = entry.index;

        drop(entries);

        let enumerates = FAKE_UDEV_ENUMERATES.lock().unwrap();
        if let Some(enumerate) = enumerates.get(&enum_ptr) {
            if let Some(device) = enumerate.devices.get(index) {
                debug!(
                    "[UDEV] udev_list_entry_get_name: returning {}",
                    device.syspath
                );
                return cache_cstring(device.syspath.clone());
            }
        }
    }
    ptr::null()
}

/// Intercept udev_device_new_from_syspath()
#[unsafe(no_mangle)]
pub unsafe extern "C" fn udev_device_new_from_syspath(
    udev: *mut c_void,
    syspath: *const c_char,
) -> *mut c_void {
    if syspath.is_null() {
        return ptr::null_mut();
    }

    let syspath_str = unsafe { CStr::from_ptr(syspath).to_str().unwrap_or("") };
    debug!(
        "[UDEV] udev_device_new_from_syspath: syspath={}",
        syspath_str
    );

    // Find the device with this syspath
    let enumerates = FAKE_UDEV_ENUMERATES.lock().unwrap();
    for enumerate in enumerates.values() {
        if let Some(device) = enumerate.devices.iter().find(|d| d.syspath == syspath_str) {
            let device_ptr = next_ptr();
            FAKE_UDEV_DEVICES
                .lock()
                .unwrap()
                .insert(device_ptr, device.clone());

            debug!(
                "[UDEV] udev_device_new_from_syspath: returning device {:x}",
                device_ptr
            );
            return device_ptr as *mut c_void;
        }
    }
    ptr::null_mut()
}

/// Intercept udev_device_get_devnode()
#[unsafe(no_mangle)]
pub unsafe extern "C" fn udev_device_get_devnode(udev_device: *mut c_void) -> *const c_char {
    let device_ptr = udev_device as usize;

    let devices = FAKE_UDEV_DEVICES.lock().unwrap();
    if let Some(device) = devices.get(&device_ptr) {
        debug!(
            "[UDEV] udev_device_get_devnode: returning {}",
            device.devnode
        );
        return cache_cstring(device.devnode.clone());
    }
    ptr::null()
}

/// Intercept udev_device_get_property_value()
#[unsafe(no_mangle)]
pub unsafe extern "C" fn udev_device_get_property_value(
    udev_device: *mut c_void,
    key: *const c_char,
) -> *const c_char {
    if key.is_null() {
        return ptr::null();
    }

    let device_ptr = udev_device as usize;
    let key_str = unsafe { CStr::from_ptr(key).to_str().unwrap_or("") };

    let devices = FAKE_UDEV_DEVICES.lock().unwrap();
    if let Some(device) = devices.get(&device_ptr) {
        if let Some(value) = device.properties.get(key_str) {
            debug!(
                "[UDEV] udev_device_get_property_value: {}={}",
                key_str, value
            );
            return cache_cstring(value.clone());
        }
    }
    ptr::null()
}

/// Parse a udev netlink-style message into a FakeUdevDevice
fn parse_udev_message(message: &str) -> Option<FakeUdevDevice> {
    let mut properties = HashMap::new();
    let mut devname = String::new();
    let mut devpath = String::new();
    let mut subsystem = String::new();
    let mut syspath = String::new();

    for line in message.lines() {
        if line.is_empty() {
            break; // Empty line terminates message
        }

        if let Some((key, value)) = line.split_once('=') {
            match key {
                "DEVNAME" => devname = value.to_string(),
                "DEVPATH" => devpath = value.to_string(),
                "SUBSYSTEM" => subsystem = value.to_string(),
                "ACTION" => {
                    debug!("[UDEV] Device action: {}", value);
                }
                _ => {
                    properties.insert(key.to_string(), value.to_string());
                }
            }
        }
    }

    if devname.is_empty() {
        debug!("[UDEV] No DEVNAME in message");
        return None;
    }

    // Construct syspath if not provided
    if syspath.is_empty() && !devpath.is_empty() {
        let base_path = crate::syscalls::get_base_path();
        syspath = format!("{}/sysfs{}", base_path, devpath);
    }

    debug!(
        "[UDEV] Parsed device: devname={}, subsystem={}",
        devname, subsystem
    );

    Some(FakeUdevDevice {
        syspath,
        devnode: devname,
        subsystem,
        properties,
    })
}
