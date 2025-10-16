use crate::ORIGINAL_FUNCTIONS;
use libc::{c_int, c_uint};
use parking_lot::Mutex;
use std::collections::{HashMap, HashSet};
use std::os::fd::AsRawFd;
use std::os::unix::io::RawFd;
use std::os::unix::net::UnixStream;
use std::sync::Arc;
use tracing::{debug, trace};
use vimputti::*;

lazy_static::lazy_static! {
    // Track which FDs are our virtual device sockets
    static ref VIRTUAL_DEVICE_FDS: Mutex<HashMap<RawFd, DeviceInfo>> = Mutex::new(HashMap::new());
    // Track which FDs are uinput emulator connections
    static ref UINPUT_FDS: Mutex<HashMap<RawFd, Arc<Mutex<UinputConnection>>>> = Mutex::new(HashMap::new());
    // Track which FDs are udev connections
    static ref UDEV_MONITOR_FDS: Mutex<HashSet<RawFd>> = Mutex::new(HashSet::new());
    // Track Unix domain sockets (to intercept connect() calls for netlink)
    static ref UNIX_SOCKET_FDS: Mutex<HashSet<RawFd>> = Mutex::new(HashSet::new());
}

struct UinputConnection {
    stream: UnixStream,
}

#[derive(Clone)]
struct DeviceInfo {
    event_node: String,
    is_joystick: bool,
    config: DeviceConfig,
}
impl DeviceInfo {
    fn num_axes(&self) -> u8 {
        self.config.axes.len() as u8
    }

    fn num_buttons(&self) -> u8 {
        self.config.buttons.len() as u8
    }

    fn device_name(&self) -> &str {
        &self.config.name
    }
}

pub(crate) fn get_all_device_configs() -> Vec<(String, DeviceConfig)> {
    VIRTUAL_DEVICE_FDS
        .lock()
        .values()
        .map(|info| (info.event_node.clone(), info.config.clone()))
        .collect()
}

pub(crate) fn get_base_path() -> String {
    "/tmp/vimputti".to_string()
}

/// Open a device node (actually connect to Unix socket)
pub fn open_device_node(socket_path: &str, _flags: c_int) -> c_int {
    use std::io::Read;
    use std::os::unix::io::IntoRawFd;
    use std::os::unix::net::UnixStream;

    debug!("Opening device node: {}", socket_path);

    match UnixStream::connect(socket_path) {
        Ok(mut stream) => {
            // Check if this is the uinput socket
            if socket_path.ends_with("/uinput") {
                let fd = stream.as_raw_fd();

                let connection = UinputConnection { stream };

                UINPUT_FDS
                    .lock()
                    .insert(fd, Arc::new(Mutex::new(connection)));

                debug!("Opened uinput emulator: fd={}", fd);
                return fd;
            }

            // Extract event node name from path
            let event_node = socket_path
                .split('/')
                .last()
                .unwrap_or("unknown")
                .to_string();

            // Check if this is a joystick device
            let is_joystick = event_node.starts_with("js");

            // Receive device config from daemon
            // Format: 4-byte length prefix + JSON config
            let mut len_buf = [0u8; 4];
            let config = match stream.read_exact(&mut len_buf) {
                Ok(_) => {
                    let config_len = u32::from_le_bytes(len_buf) as usize;
                    debug!("Receiving device config ({} bytes)", config_len);

                    let mut config_buf = vec![0u8; config_len];
                    match stream.read_exact(&mut config_buf) {
                        Ok(_) => match serde_json::from_slice::<DeviceConfig>(&config_buf) {
                            Ok(config) => {
                                debug!("Successfully received device config: {}", config.name);
                                config
                            }
                            Err(e) => {
                                debug!("Failed to deserialize device config: {}, using default", e);
                                vimputti::templates::ControllerTemplates::xbox360()
                            }
                        },
                        Err(e) => {
                            debug!("Failed to read device config data: {}, using default", e);
                            vimputti::templates::ControllerTemplates::xbox360()
                        }
                    }
                }
                Err(e) => {
                    debug!("Failed to read config length: {}, using default", e);
                    vimputti::templates::ControllerTemplates::xbox360()
                }
            };

            let fd = stream.into_raw_fd();

            // Register this FD as a virtual device
            VIRTUAL_DEVICE_FDS.lock().insert(
                fd,
                DeviceInfo {
                    event_node: event_node.clone(),
                    is_joystick,
                    config: config.clone(),
                },
            );

            debug!(
                "Opened virtual device: fd={}, node={}, is_joystick={}, buttons={}, axes={}",
                fd,
                event_node,
                is_joystick,
                config.buttons.len(),
                config.axes.len()
            );
            fd
        }
        Err(e) => {
            debug!("Failed to connect to device socket {}: {}", socket_path, e);
            -1
        }
    }
}

/// Check if an FD is one of our virtual devices
pub fn is_virtual_device_fd(fd: RawFd) -> bool {
    VIRTUAL_DEVICE_FDS.lock().contains_key(&fd)
}

/// Check if an FD is a uinput emulator FD
pub fn is_uinput_fd(fd: RawFd) -> bool {
    UINPUT_FDS.lock().contains_key(&fd)
}

pub fn register_udev_monitor_fd(fd: RawFd) {
    UDEV_MONITOR_FDS.lock().insert(fd);
    debug!("Registered udev monitor fd: {}", fd);
}

pub fn is_udev_monitor_fd(fd: RawFd) -> bool {
    UDEV_MONITOR_FDS.lock().contains(&fd)
}

/// Handle ioctl() calls on virtual device FDs
pub unsafe fn handle_ioctl(fd: RawFd, request: c_uint, args: &mut std::ffi::VaListImpl) -> c_int {
    // Get device info
    let device_fds = VIRTUAL_DEVICE_FDS.lock();
    let device_info = device_fds.get(&fd).cloned();
    drop(device_fds);

    if let Some(info) = device_info {
        if info.is_joystick {
            return unsafe { handle_joystick_ioctl(fd, request, args, &info) };
        }
        return unsafe { handle_evdev_ioctl(fd, request, args, &info) };
    }

    -1
}

/// Handle joystick interface ioctl calls
unsafe fn handle_joystick_ioctl(
    _fd: RawFd,
    request: u32,
    args: &mut std::ffi::VaListImpl,
    device_info: &DeviceInfo,
) -> c_int {
    // Joystick interface ioctl constants
    const JSIOCGVERSION: u32 = 0x80046a01;
    const JSIOCGAXES: u32 = 0x80016a11;
    const JSIOCGBUTTONS: u32 = 0x80016a12;
    const JSIOCGNAME_BASE: u32 = 0x80006a13;
    const JSIOCGBTNMAP: u32 = 0x80406a34;
    const JSIOCGAXMAP: u32 = 0x80406a32;

    let request_type = (request >> 8) & 0xFF;
    let request_nr = request & 0xFF;

    match request {
        JSIOCGVERSION => {
            let ptr: *mut c_int = unsafe { args.arg() };
            if !ptr.is_null() {
                unsafe {
                    *ptr = 0x020100; // Version 2.1.0
                }
            }
            0
        }

        JSIOCGAXES => {
            let ptr: *mut u8 = unsafe { args.arg() };
            if !ptr.is_null() {
                unsafe {
                    *ptr = device_info.num_axes();
                }
            }
            0
        }

        JSIOCGBUTTONS => {
            let ptr: *mut u8 = unsafe { args.arg() };
            if !ptr.is_null() {
                unsafe {
                    *ptr = device_info.num_buttons();
                }
            }
            0
        }

        JSIOCGAXMAP => {
            let ptr: *mut u8 = unsafe { args.arg() };
            let len = ((request >> 16) & 0x3FFF) as usize;

            if !ptr.is_null() && len > 0 {
                // Build axis map from device config
                // Map joystick axis number (index) to evdev axis code
                let mut axis_map = Vec::new();
                for axis_config in &device_info.config.axes {
                    axis_map.push(axis_config.axis.to_ev_code() as u8);
                }

                let copy_len = std::cmp::min(axis_map.len(), len);
                unsafe {
                    std::ptr::copy_nonoverlapping(axis_map.as_ptr(), ptr, copy_len);
                }
            }
            0
        }

        JSIOCGBTNMAP => {
            let ptr: *mut u16 = unsafe { args.arg() };
            let len = ((request >> 16) & 0x3FFF) as usize / 2;

            if !ptr.is_null() && len > 0 {
                // Build button map from device config
                // Map joystick button number (index) to evdev button code
                let mut button_map = Vec::new();
                for button in &device_info.config.buttons {
                    button_map.push(button.to_ev_code());
                }

                let copy_len = std::cmp::min(button_map.len(), len);
                unsafe {
                    std::ptr::copy_nonoverlapping(button_map.as_ptr(), ptr, copy_len);
                }
            }
            0
        }

        _ if request_type == 0x6a && request_nr == 0x13 => {
            let ptr: *mut u8 = unsafe { args.arg() };
            let len = ((request >> 16) & 0xFF) as usize;

            if !ptr.is_null() && len > 0 {
                let name_bytes = device_info.device_name().as_bytes();
                let copy_len = std::cmp::min(name_bytes.len(), len - 1);
                unsafe {
                    std::ptr::copy_nonoverlapping(name_bytes.as_ptr(), ptr, copy_len);
                }
                unsafe {
                    *ptr.add(copy_len) = 0;
                } // Null terminator
                copy_len as c_int
            } else {
                -1
            }
        }

        _ => {
            debug!("ioctl: unknown joystick request 0x{:08x}", request);
            0
        }
    }
}

/// Handle evdev interface ioctl calls
unsafe fn handle_evdev_ioctl(
    _fd: RawFd,
    request: c_uint,
    args: &mut std::ffi::VaListImpl,
    device_info: &DeviceInfo,
) -> c_int {
    const EVIOCGVERSION: c_uint = 0x80044501;
    const EVIOCGID: c_uint = 0x80084502;

    // evdev ioctl request number ranges
    const EVIOCG_TYPE_MASK: u32 = 0xFF;
    const EVIOCG_NR_MASK: u32 = 0xFF;
    const EVIOCG_SIZE_SHIFT: u32 = 16;
    const EVIOCG_SIZE_MASK: u32 = 0x3FFF;

    // Request type for evdev ioctls
    const EVDEV_IOC_TYPE: u32 = b'E' as u32;

    // evdev ioctl number ranges
    const EVIOCGBIT_NR_BASE: u32 = 0x20;
    const EVIOCGBIT_NR_END: u32 = 0x40;
    const EVIOCGABS_NR_BASE: u32 = 0x40;
    const EVIOCGABS_NR_END: u32 = 0x80;

    // Helper to extract ioctl components
    fn extract_request_type(request: u32) -> u32 {
        (request >> 8) & EVIOCG_TYPE_MASK
    }

    fn extract_request_nr(request: u32) -> u32 {
        request & EVIOCG_NR_MASK
    }

    fn extract_request_size(request: u32) -> usize {
        ((request >> EVIOCG_SIZE_SHIFT) & EVIOCG_SIZE_MASK) as usize
    }

    let request_nr = extract_request_nr(request);

    match request {
        EVIOCGVERSION => {
            let ptr: *mut c_int = unsafe { args.arg() };
            if !ptr.is_null() {
                unsafe {
                    *ptr = 0x010001;
                }
            }
            0
        }

        EVIOCGID => {
            #[repr(C)]
            struct InputId {
                bustype: u16,
                vendor: u16,
                product: u16,
                version: u16,
            }

            let ptr: *mut InputId = unsafe { args.arg() };
            if !ptr.is_null() {
                unsafe {
                    *ptr = InputId {
                        bustype: device_info.config.bustype as u16,
                        vendor: device_info.config.vendor_id,
                        product: device_info.config.product_id,
                        version: device_info.config.version,
                    };
                }
            }
            0
        }

        // EVIOCGNAME - get device name
        _ if extract_request_type(request) == EVDEV_IOC_TYPE && request_nr == 0x06 => {
            let ptr: *mut u8 = unsafe { args.arg() };
            let len = extract_request_size(request);

            if !ptr.is_null() && len > 0 {
                let name_bytes = device_info.device_name().as_bytes();
                let copy_len = std::cmp::min(name_bytes.len(), len - 1);
                unsafe {
                    std::ptr::copy_nonoverlapping(name_bytes.as_ptr(), ptr, copy_len);
                    *ptr.add(copy_len) = 0;
                }
                copy_len as c_int
            } else {
                -1
            }
        }

        // EVIOCGPHYS - get physical location
        _ if extract_request_type(request) == EVDEV_IOC_TYPE && request_nr == 0x07 => {
            let ptr: *mut u8 = unsafe { args.arg() };
            let len = extract_request_size(request);

            if !ptr.is_null() && len > 0 {
                let phys = b"vimputti-virtual\0";
                let copy_len = std::cmp::min(phys.len(), len);
                unsafe {
                    std::ptr::copy_nonoverlapping(phys.as_ptr(), ptr, copy_len);
                }
                copy_len as c_int
            } else {
                -1
            }
        }

        // EVIOCGUNIQ - get unique identifier
        _ if extract_request_type(request) == EVDEV_IOC_TYPE && request_nr == 0x08 => {
            let ptr: *mut u8 = unsafe { args.arg() };
            let len = extract_request_size(request);

            if !ptr.is_null() && len > 0 {
                unsafe {
                    *ptr = 0;
                }
                1
            } else {
                -1
            }
        }

        // EVIOCGPROP - get device properties
        _ if extract_request_type(request) == EVDEV_IOC_TYPE && request_nr == 0x09 => {
            let ptr: *mut u8 = unsafe { args.arg() };
            let len = extract_request_size(request);

            if !ptr.is_null() && len > 0 {
                unsafe {
                    std::ptr::write_bytes(ptr, 0, len);
                }
                0
            } else {
                -1
            }
        }

        // EVIOCGBIT(ev, len) - get event bits for specific event type
        _ if extract_request_type(request) == EVDEV_IOC_TYPE
            && request_nr >= EVIOCGBIT_NR_BASE
            && request_nr < EVIOCGBIT_NR_END =>
        {
            let ev_type = request_nr - EVIOCGBIT_NR_BASE;
            let ptr: *mut u8 = unsafe { args.arg() };
            let len = extract_request_size(request);

            if !ptr.is_null() && len > 0 {
                // Clear buffer
                unsafe {
                    std::ptr::write_bytes(ptr, 0, len);
                }

                // Set bits based on device config
                match ev_type as u16 {
                    0 => {
                        if len > 0 {
                            unsafe {
                                *ptr = 0b00001011;
                            }
                        }
                    }
                    EV_KEY => {
                        for button in &device_info.config.buttons {
                            let code = button.to_ev_code() as usize;
                            unsafe {
                                *ptr.add(code / 8) |= 1 << (code % 8);
                            }
                        }
                    }
                    EV_REL => {
                        // No relative axes in our virtual devices..
                    }
                    EV_ABS => {
                        for axis in &device_info.config.axes {
                            let code = axis.axis.to_ev_code() as usize;
                            unsafe {
                                *ptr.add(code / 8) |= 1 << (code % 8);
                            }
                        }
                    }
                    _ => {
                        debug!("ioctl EVIOCGBIT({}): unknown type", ev_type);
                    }
                }
                0
            } else {
                -1
            }
        }

        // EVIOCGABS(abs) - get abs axis info
        _ if extract_request_type(request) == EVDEV_IOC_TYPE
            && request_nr >= EVIOCGABS_NR_BASE
            && request_nr < EVIOCGABS_NR_END =>
        {
            let axis_code = request_nr - EVIOCGABS_NR_BASE;
            let ptr: *mut LinuxAbsEvent = unsafe { args.arg() };

            if !ptr.is_null() {
                // Try to find the axis in the device config
                let axis_info = {
                    device_info
                        .config
                        .axes
                        .iter()
                        .find(|a| a.axis.to_ev_code() as u32 == axis_code)
                        .map(|a| LinuxAbsEvent {
                            value: 0,
                            minimum: a.min,
                            maximum: a.max,
                            fuzz: if a.max > 1000 { 16 } else { 0 },
                            flat: if a.max > 1000 { 128 } else { 0 },
                            resolution: 0,
                        })
                };

                // Fallback to defaults if not found
                unsafe {
                    *ptr = axis_info.unwrap_or(LinuxAbsEvent {
                        value: 0,
                        minimum: -32768,
                        maximum: 32767,
                        fuzz: 16,
                        flat: 128,
                        resolution: 0,
                    });
                }
                0
            } else {
                -1
            }
        }
        _ => {
            debug!("ioctl: unknown request 0x{:08x} on virtual device", request);
            -1
        }
    }
}

/// Clean up when a virtual device FD is closed
pub fn close_virtual_device(fd: RawFd) {
    VIRTUAL_DEVICE_FDS.lock().remove(&fd);
    UINPUT_FDS.lock().remove(&fd);
    UDEV_MONITOR_FDS.lock().remove(&fd);
    UNIX_SOCKET_FDS.lock().remove(&fd);
}

// Helper to send uinput request and get response
fn send_uinput_request(fd: RawFd, request: vimputti::protocol::UinputRequest) -> c_int {
    use std::io::{Read, Write};

    let connection_arc = {
        let uinput_fds = UINPUT_FDS.lock();
        match uinput_fds.get(&fd) {
            Some(c) => c.clone(),
            None => {
                debug!("uinput fd {} not found", fd);
                return -1;
            }
        }
    };

    let mut connection = connection_arc.lock();

    // Serialize request with length prefix
    let request_bytes = match request.to_bytes() {
        Ok(b) => b,
        Err(e) => {
            debug!("Failed to serialize request: {}", e);
            return -1;
        }
    };

    trace!("Sending {} bytes to uinput fd={}", request_bytes.len(), fd);

    // Send request (4-byte length prefix + JSON)
    if let Err(e) = connection.stream.write_all(&request_bytes) {
        debug!("Failed to write request to fd={}: {}", fd, e);
        return -1;
    }
    if let Err(e) = connection.stream.flush() {
        debug!("Failed to flush fd={}: {}", fd, e);
        return -1;
    }

    // Read response - 4-byte length prefix first
    let mut len_buf = [0u8; 4];
    match connection.stream.read_exact(&mut len_buf) {
        Ok(_) => {}
        Err(e) => {
            debug!("Failed to read response length from fd={}: {}", fd, e);
            return -1;
        }
    }

    let response_len = u32::from_le_bytes(len_buf) as usize;
    if response_len == 0 || response_len > 1_000_000 {
        debug!("Invalid response length: {} from fd={}", response_len, fd);
        return -1;
    }

    trace!("Reading {} byte response from fd={}", response_len, fd);

    // Read response body
    let mut response_buf = vec![0u8; response_len];
    match connection.stream.read_exact(&mut response_buf) {
        Ok(_) => {}
        Err(e) => {
            debug!("Failed to read response body from fd={}: {}", fd, e);
            return -1;
        }
    }

    let response: vimputti::protocol::UinputResponse =
        match vimputti::protocol::UinputResponse::from_bytes(&response_buf) {
            Ok(resp) => resp,
            Err(e) => {
                debug!("Failed to parse response from fd={}: {}", fd, e);
                return -1;
            }
        };

    trace!("Response from fd={}: success={}", fd, response.success);

    if response.success { 0 } else { -1 }
}

/// Handle uinput ioctl calls
pub unsafe fn handle_uinput_ioctl(
    fd: RawFd,
    request: c_uint,
    args: &mut std::ffi::VaListImpl,
) -> c_int {
    const UI_SET_EVBIT: c_uint = 0x40045564;
    const UI_SET_KEYBIT: c_uint = 0x40045565;
    const UI_SET_RELBIT: c_uint = 0x40045566;
    const UI_SET_ABSBIT: c_uint = 0x40045567;
    const UI_SET_MSCBIT: c_uint = 0x40045568;
    const UI_SET_LEDBIT: c_uint = 0x40045569;
    const UI_SET_SNDBIT: c_uint = 0x4004556a;
    const UI_SET_FFBIT: c_uint = 0x4004556b;
    const UI_SET_PHYS: c_uint = 0x4004556c;
    const UI_SET_SWBIT: c_uint = 0x4004556d;
    const UI_SET_PROPBIT: c_uint = 0x4004556e;

    const UI_DEV_SETUP: c_uint = 0x405c5503;
    const UI_DEV_CREATE: c_uint = 0x5501;
    const UI_DEV_DESTROY: c_uint = 0x5502;
    const UI_ABS_SETUP: c_uint = 0x401c5504;

    const FIONREAD: c_uint = 0x5421;

    debug!("uinput ioctl: fd={}, request=0x{:x}", fd, request);

    match request {
        UI_SET_EVBIT => {
            let ev_type: c_uint = unsafe { args.arg() };
            send_uinput_request(
                fd,
                vimputti::protocol::UinputRequest::SetEvBit {
                    ev_type: ev_type as u16,
                },
            )
        }

        UI_SET_KEYBIT => {
            let key_code: c_uint = unsafe { args.arg() };
            send_uinput_request(
                fd,
                vimputti::protocol::UinputRequest::SetKeyBit {
                    key_code: key_code as u16,
                },
            )
        }

        UI_SET_ABSBIT => {
            let abs_code: c_uint = unsafe { args.arg() };
            send_uinput_request(
                fd,
                vimputti::protocol::UinputRequest::SetAbsBit {
                    abs_code: abs_code as u16,
                },
            )
        }

        UI_SET_RELBIT => {
            let rel_code: c_uint = unsafe { args.arg() };
            send_uinput_request(
                fd,
                vimputti::protocol::UinputRequest::SetRelBit {
                    rel_code: rel_code as u16,
                },
            )
        }

        UI_ABS_SETUP => {
            #[repr(C)]
            struct UiAbsSetup {
                code: u16,
                absinfo: vimputti::LinuxAbsEvent,
            }
            let ptr: *const UiAbsSetup = unsafe { args.arg() };
            if !ptr.is_null() {
                let setup = unsafe { &*ptr };
                send_uinput_request(
                    fd,
                    vimputti::protocol::UinputRequest::AbsSetup {
                        code: setup.code,
                        absinfo: setup.absinfo,
                    },
                )
            } else {
                0
            }
        }

        UI_DEV_SETUP => {
            #[repr(C)]
            struct UiSetup {
                id: [u16; 4], // bustype, vendor, product, version
                name: [u8; 80],
                ff_effects_max: u32,
            }
            let ptr: *const UiSetup = unsafe { args.arg() };
            if !ptr.is_null() {
                let setup = unsafe { &*ptr };
                let name = std::ffi::CStr::from_bytes_until_nul(&setup.name)
                    .ok()
                    .and_then(|s| s.to_str().ok())
                    .unwrap_or("virtual uinput device")
                    .to_string();

                send_uinput_request(
                    fd,
                    vimputti::protocol::UinputRequest::DevSetup {
                        setup: vimputti::protocol::DeviceSetup {
                            name,
                            vendor_id: setup.id[1],
                            product_id: setup.id[2],
                            version: setup.id[3],
                            bustype: setup.id[0],
                        },
                    },
                )
            } else {
                0
            }
        }

        UI_DEV_CREATE => send_uinput_request(fd, vimputti::protocol::UinputRequest::DevCreate {}),

        UI_DEV_DESTROY => send_uinput_request(fd, vimputti::protocol::UinputRequest::DevDestroy {}),

        UI_SET_MSCBIT | UI_SET_LEDBIT | UI_SET_SNDBIT | UI_SET_FFBIT | UI_SET_SWBIT
        | UI_SET_PROPBIT | UI_SET_PHYS => {
            // Ignore these for now
            0
        }

        FIONREAD => {
            // Return 0 bytes available (no data to read from uinput)
            let ptr: *mut c_int = unsafe { args.arg() };
            if !ptr.is_null() {
                unsafe {
                    *ptr = 0;
                }
            }
            0
        }

        _ => {
            debug!("[UINPUT] Unknown ioctl request 0x{:x}", request);
            0
        }
    }
}

/// Handle write() calls on uinput FDs
pub unsafe fn handle_uinput_write(
    fd: RawFd,
    buf: *const libc::c_void,
    count: libc::size_t,
) -> libc::ssize_t {
    use std::io::Write;
    use std::slice;

    if buf.is_null() || count == 0 {
        return 0;
    }

    let event_size_64 = std::mem::size_of::<vimputti::protocol::LinuxInputEvent>();
    let event_size_32 = 16;

    // Parse events
    let events = if count % event_size_64 == 0 {
        let num_events = count / event_size_64;
        let events_slice = unsafe {
            slice::from_raw_parts(
                buf as *const vimputti::protocol::LinuxInputEvent,
                num_events,
            )
        };
        events_slice.to_vec()
    } else if count % event_size_32 == 0 {
        let num_events = count / event_size_32;
        #[repr(C, packed)]
        struct InputEvent32 {
            tv_sec: i32,
            tv_usec: i32,
            type_: u16,
            code: u16,
            value: i32,
        }
        let events_slice_32 =
            unsafe { slice::from_raw_parts(buf as *const InputEvent32, num_events) };
        events_slice_32
            .iter()
            .map(|e| vimputti::protocol::LinuxInputEvent {
                time: vimputti::TimeVal {
                    tv_sec: e.tv_sec as i64,
                    tv_usec: e.tv_usec as i64,
                },
                event_type: e.type_,
                code: e.code,
                value: e.value,
            })
            .collect()
    } else {
        trace!("uinput write: unknown format size {}", count);
        return count as libc::ssize_t;
    };

    // Get connection
    let connection_arc = {
        let uinput_fds = UINPUT_FDS.lock();
        match uinput_fds.get(&fd) {
            Some(c) => c.clone(),
            None => return count as libc::ssize_t, // Pretend success
        }
    };

    let mut connection = connection_arc.lock();
    let events_len = events.len();

    // Create WriteEvents request
    let request = vimputti::protocol::UinputRequest::WriteEvents { events };

    // Serialize and send
    if let Ok(request_bytes) = request.to_bytes() {
        trace!(
            "Sending {} events ({} bytes) - fire and forget",
            events_len,
            request_bytes.len()
        );

        // Just write and return immediately
        let _ = connection.stream.write_all(&request_bytes);
        let _ = connection.stream.flush();
    }

    // Return success immediately without waiting
    count as libc::ssize_t
}

/* netlink unix sockets */

/// Track that an FD is a Unix domain socket
pub fn track_unix_socket(fd: RawFd) {
    UNIX_SOCKET_FDS.lock().insert(fd);
    trace!("Tracked Unix socket fd: {}", fd);
}

/// Check if FD is a tracked Unix socket
pub fn is_tracked_unix_socket(fd: RawFd) -> bool {
    UNIX_SOCKET_FDS.lock().contains(&fd)
}
