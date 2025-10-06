use libc::{c_int, c_uint};
use std::collections::HashMap;
use std::os::unix::io::RawFd;
use std::sync::Mutex;
use tracing::{debug, warn};

lazy_static::lazy_static! {
    // Track which FDs are our virtual device sockets
    static ref VIRTUAL_DEVICE_FDS: Mutex<HashMap<RawFd, DeviceInfo>> = Mutex::new(HashMap::new());
}

#[derive(Clone)]
struct DeviceInfo {
    event_node: String,
    is_joystick: bool,
    num_buttons: u8,
    num_axes: u8,
    device_name: String,
}

/// Open a device node (actually connect to Unix socket)
pub fn open_device_node(socket_path: &str, _flags: c_int) -> c_int {
    use std::os::unix::io::IntoRawFd;
    use std::os::unix::net::UnixStream;

    debug!("Opening device node: {}", socket_path);

    match UnixStream::connect(socket_path) {
        Ok(stream) => {
            let fd = stream.into_raw_fd();

            // Extract event node name from path
            let event_node = socket_path
                .split('/')
                .last()
                .unwrap_or("unknown")
                .to_string();

            // Check if this is a joystick device
            let is_joystick = event_node.starts_with("js");

            // Try to read device info from sysfs
            let (num_buttons, num_axes, device_name) = if is_joystick {
                read_device_info_from_sysfs(&event_node)
            } else {
                (11, 6, "Virtual Controller".to_string())
            };

            // Register this FD as a virtual device
            VIRTUAL_DEVICE_FDS.lock().unwrap().insert(
                fd,
                DeviceInfo {
                    event_node: event_node.clone(),
                    is_joystick,
                    num_buttons,
                    num_axes,
                    device_name,
                },
            );

            debug!(
                "Opened virtual device: fd={}, node={}, is_joystick={}, buttons={}, axes={}",
                fd, event_node, is_joystick, num_buttons, num_axes
            );
            fd
        }
        Err(e) => {
            warn!("Failed to connect to device socket {}: {}", socket_path, e);
            -1
        }
    }
}

/// Read device info from sysfs (for proper button/axis counts)
fn read_device_info_from_sysfs(event_node: &str) -> (u8, u8, String) {
    let uid = unsafe { libc::getuid() };
    let base_path =
        std::env::var("VIMPUTTI_PATH").unwrap_or_else(|_| format!("/run/user/{}/vimputti", uid));

    // Map js0 -> event0
    let event_num = event_node.trim_start_matches("js");
    let sysfs_event_node = format!("event{}", event_num);

    let sysfs_base = format!(
        "{}/sysfs/class/input/{}/device",
        base_path, sysfs_event_node
    );

    // Read device name
    let device_name = std::fs::read_to_string(format!("{}/name", sysfs_base))
        .unwrap_or_else(|_| "Virtual Controller".to_string())
        .trim()
        .to_string();

    // Read capabilities to count buttons and axes
    let key_caps = std::fs::read_to_string(format!("{}/capabilities/key", sysfs_base))
        .unwrap_or_else(|_| "0".to_string());
    let abs_caps = std::fs::read_to_string(format!("{}/capabilities/abs", sysfs_base))
        .unwrap_or_else(|_| "0".to_string());

    let num_buttons = count_bits_in_hex(&key_caps);
    let num_axes = count_bits_in_hex(&abs_caps);

    debug!(
        "Read from sysfs: buttons={}, axes={}, name={}",
        num_buttons, num_axes, device_name
    );

    (num_buttons, num_axes, device_name)
}

/// Count set bits in a hex string (e.g., "3f" = 6 bits)
fn count_bits_in_hex(hex_str: &str) -> u8 {
    hex_str
        .split_whitespace()
        .filter_map(|s| u64::from_str_radix(s, 16).ok())
        .map(|n| n.count_ones() as u8)
        .sum()
}

/// Check if an FD is one of our virtual devices
pub fn is_virtual_device_fd(fd: RawFd) -> bool {
    VIRTUAL_DEVICE_FDS.lock().unwrap().contains_key(&fd)
}

/// Handle ioctl() calls on virtual device FDs
pub unsafe fn handle_ioctl(fd: RawFd, request: c_uint, args: &mut std::ffi::VaListImpl) -> c_int {
    debug!(
        "ioctl on fd={}, request=0x{:08x} (type={}, nr={}, size={})",
        fd,
        request,
        (request >> 8) & 0xFF,
        request & 0xFF,
        (request >> 16) & 0x3FFF
    );

    // Get device info
    let device_fds = VIRTUAL_DEVICE_FDS.lock().unwrap();
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
                    *ptr = 0x020100;
                }
                debug!("ioctl JSIOCGVERSION: returning 0x020100");
            }
            0
        }

        JSIOCGAXES => {
            let ptr: *mut u8 = unsafe { args.arg() };
            if !ptr.is_null() {
                unsafe {
                    *ptr = device_info.num_axes;
                }
                debug!("ioctl JSIOCGAXES: returning {} axes", device_info.num_axes);
            }
            0
        }

        JSIOCGBUTTONS => {
            let ptr: *mut u8 = unsafe { args.arg() };
            if !ptr.is_null() {
                unsafe {
                    *ptr = device_info.num_buttons;
                }
                debug!(
                    "ioctl JSIOCGBUTTONS: returning {} buttons",
                    device_info.num_buttons
                );
            }
            0
        }

        JSIOCGAXMAP => {
            let ptr: *mut u8 = unsafe { args.arg() };
            let len = ((request >> 16) & 0x3FFF) as usize;

            if !ptr.is_null() && len > 0 {
                // Axis mapping: joystick axis number -> Linux ABS_* code
                let axis_map: [u8; 8] = [
                    0,  // Axis 0: ABS_X (LeftStickX)
                    1,  // Axis 1: ABS_Y (LeftStickY)
                    3,  // Axis 2: ABS_RX (RightStickX)
                    4,  // Axis 3: ABS_RY (RightStickY)
                    2,  // Axis 4: ABS_Z (LeftTrigger)
                    5,  // Axis 5: ABS_RZ (RightTrigger)
                    16, // Axis 6: ABS_HAT0X (DPadX)
                    17, // Axis 7: ABS_HAT0Y (DPadY)
                ];
                let copy_len = std::cmp::min(axis_map.len(), len);
                unsafe {
                    std::ptr::copy_nonoverlapping(axis_map.as_ptr(), ptr, copy_len);
                }
                debug!("ioctl JSIOCGAXMAP: returning axis map");
            }
            0
        }

        JSIOCGBTNMAP => {
            let ptr: *mut u16 = unsafe { args.arg() };
            let len = ((request >> 16) & 0x3FFF) as usize / 2;

            if !ptr.is_null() && len > 0 {
                let button_map: [u16; 15] = [
                    304, // Button 0: BTN_SOUTH (A)
                    305, // Button 1: BTN_EAST (B)
                    307, // Button 2: BTN_NORTH (X)
                    308, // Button 3: BTN_WEST (Y)
                    310, // Button 4: BTN_TL (LeftBumper)
                    311, // Button 5: BTN_TR (RightBumper)
                    312, // Button 6: BTN_TL2 (LeftTrigger button)
                    313, // Button 7: BTN_TR2 (RightTrigger button)
                    314, // Button 8: BTN_SELECT
                    315, // Button 9: BTN_START
                    316, // Button 10: BTN_MODE (Guide)
                    317, // Button 11: BTN_THUMBL (LeftStick)
                    318, // Button 12: BTN_THUMBR (RightStick)
                    544, // Button 13: BTN_DPAD_UP
                    545, // Button 14: BTN_DPAD_DOWN
                ];
                let copy_len = std::cmp::min(button_map.len(), len);
                unsafe {
                    std::ptr::copy_nonoverlapping(button_map.as_ptr(), ptr, copy_len);
                }
                debug!("ioctl JSIOCGBTNMAP: returning button map");
            }
            0
        }

        _ if request_type == 0x6a && request_nr == 0x13 => {
            let ptr: *mut u8 = unsafe { args.arg() };
            let len = ((request >> 16) & 0xFF) as usize;

            if !ptr.is_null() && len > 0 {
                let name_bytes = device_info.device_name.as_bytes();
                let copy_len = std::cmp::min(name_bytes.len(), len - 1);
                unsafe {
                    std::ptr::copy_nonoverlapping(name_bytes.as_ptr(), ptr, copy_len);
                }
                unsafe {
                    *ptr.add(copy_len) = 0;
                } // Null terminator
                debug!("ioctl JSIOCGNAME: returning '{}'", device_info.device_name);
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

    let request_nr = request & 0xFF;
    let request_type = (request >> 8) & 0xFF;

    match request {
        EVIOCGVERSION => {
            let ptr: *mut c_int = unsafe { args.arg() };
            if !ptr.is_null() {
                unsafe {
                    *ptr = 0x010001;
                }
                debug!("ioctl EVIOCGVERSION: returning 0x010001");
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
                        bustype: 0x03,
                        vendor: 0x045e,
                        product: 0x028e,
                        version: 0x0110,
                    };
                }
                debug!("ioctl EVIOCGID: returning Xbox 360 controller ID");
            }
            0
        }

        _ if request_type == b'E' as u32 && request_nr == 0x06 => {
            let ptr: *mut u8 = unsafe { args.arg() };
            let len = ((request >> 16) & 0x1FFF) as usize;

            if !ptr.is_null() && len > 0 {
                let name_bytes = device_info.device_name.as_bytes();
                let copy_len = std::cmp::min(name_bytes.len(), len - 1);
                unsafe {
                    std::ptr::copy_nonoverlapping(name_bytes.as_ptr(), ptr, copy_len);
                }
                unsafe {
                    *ptr.add(copy_len) = 0;
                }
                debug!("ioctl EVIOCGNAME: returning '{}'", device_info.device_name);
                copy_len as c_int
            } else {
                -1
            }
        }

        _ if request_type == b'E' as u32 && request_nr == 0x07 => {
            let ptr: *mut u8 = unsafe { args.arg() };
            let len = ((request >> 16) & 0x1FFF) as usize;

            if !ptr.is_null() && len > 0 {
                let phys = b"vimputti-virtual\0";
                let copy_len = std::cmp::min(phys.len(), len);
                unsafe {
                    std::ptr::copy_nonoverlapping(phys.as_ptr(), ptr, copy_len);
                }
                debug!("ioctl EVIOCGPHYS: returning 'vimputti-virtual'");
                copy_len as c_int
            } else {
                -1
            }
        }

        _ if request_type == b'E' as u32 && request_nr == 0x08 => {
            let ptr: *mut u8 = unsafe { args.arg() };
            let len = ((request >> 16) & 0x1FFF) as usize;

            if !ptr.is_null() && len > 0 {
                unsafe {
                    *ptr = 0;
                }
                debug!("ioctl EVIOCGUNIQ: returning empty");
                1
            } else {
                -1
            }
        }

        _ if request_type == b'E' as u32 && request_nr == 0x09 => {
            let ptr: *mut u8 = unsafe { args.arg() };
            let len = ((request >> 16) & 0x1FFF) as usize;

            if !ptr.is_null() && len > 0 {
                unsafe {
                    std::ptr::write_bytes(ptr, 0, len);
                }
                debug!("ioctl EVIOCGPROP: returning empty properties");
                0
            } else {
                -1
            }
        }

        _ if request_type == b'E' as u32 && request_nr >= 0x20 && request_nr < 0x40 => {
            let ev_type = request_nr - 0x20;
            let ptr: *mut u8 = unsafe { args.arg() };
            let len = ((request >> 16) & 0x1FFF) as usize;

            if !ptr.is_null() && len > 0 {
                unsafe {
                    std::ptr::write_bytes(ptr, 0, len);
                }

                match ev_type {
                    0 => {
                        if len > 0 {
                            unsafe {
                                *ptr = 0b00001011;
                            }
                        }
                        debug!("ioctl EVIOCGBIT(0): returning supported event types");
                    }
                    1 => {
                        if len >= 40 {
                            for i in 304..=318 {
                                let byte_idx = i / 8;
                                let bit_idx = i % 8;
                                if byte_idx < len {
                                    unsafe {
                                        *ptr.add(byte_idx) |= 1 << bit_idx;
                                    }
                                }
                            }
                        }
                        debug!("ioctl EVIOCGBIT(EV_KEY): returning button bits");
                    }
                    3 => {
                        if len > 0 {
                            unsafe {
                                *ptr = 0b00111111;
                            }
                        }
                        debug!("ioctl EVIOCGBIT(EV_ABS): returning axis bits");
                    }
                    _ => {
                        debug!(
                            "ioctl EVIOCGBIT({}): unknown type, returning empty",
                            ev_type
                        );
                    }
                }
                0
            } else {
                -1
            }
        }

        _ if request_type == b'E' as u32 && request_nr >= 0x40 && request_nr < 0x80 => {
            let axis = request_nr - 0x40;

            #[repr(C)]
            struct InputAbsinfo {
                value: i32,
                minimum: i32,
                maximum: i32,
                fuzz: i32,
                flat: i32,
                resolution: i32,
            }

            let ptr: *mut InputAbsinfo = unsafe { args.arg() };
            if !ptr.is_null() {
                unsafe {
                    *ptr = match axis {
                        0..=1 => InputAbsinfo {
                            value: 0,
                            minimum: -32768,
                            maximum: 32767,
                            fuzz: 16,
                            flat: 128,
                            resolution: 0,
                        },
                        2..=5 => InputAbsinfo {
                            value: 0,
                            minimum: 0,
                            maximum: 255,
                            fuzz: 0,
                            flat: 15,
                            resolution: 0,
                        },
                        _ => InputAbsinfo {
                            value: 0,
                            minimum: -1,
                            maximum: 1,
                            fuzz: 0,
                            flat: 0,
                            resolution: 0,
                        },
                    };
                }
                debug!("ioctl EVIOCGABS({}): returning axis info", axis);
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
    if let Some(info) = VIRTUAL_DEVICE_FDS.lock().unwrap().remove(&fd) {
        debug!("Closed virtual device: fd={}, node={}", fd, info.event_node);
    }
}
