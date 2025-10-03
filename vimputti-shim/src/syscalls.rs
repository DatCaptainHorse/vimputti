use libc::{c_int, c_uint};
use std::collections::HashMap;
use std::os::unix::io::RawFd;
use std::sync::Mutex;
use tracing::{debug, warn};

lazy_static::lazy_static! {
    // Track which FDs are our virtual device sockets
    static ref VIRTUAL_DEVICE_FDS: Mutex<HashMap<RawFd, DeviceInfo>> = Mutex::new(HashMap::new());
}

struct DeviceInfo {
    event_node: String,
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

            // Register this FD as a virtual device
            VIRTUAL_DEVICE_FDS.lock().unwrap().insert(
                fd,
                DeviceInfo {
                    event_node: event_node.clone(),
                },
            );

            debug!("Opened virtual device: fd={}, node={}", fd, event_node);
            fd
        }
        Err(e) => {
            warn!("Failed to connect to device socket {}: {}", socket_path, e);
            -1
        }
    }
}

/// Check if an FD is one of our virtual devices
pub fn is_virtual_device_fd(fd: RawFd) -> bool {
    VIRTUAL_DEVICE_FDS.lock().unwrap().contains_key(&fd)
}

/// Handle ioctl() calls on virtual device FDs
pub unsafe fn handle_ioctl(_fd: RawFd, request: c_uint, args: &mut std::ffi::VaListImpl) -> c_int {
    // Linux input device ioctl constants
    const EVIOCGVERSION: c_uint = 0x80044501;
    const EVIOCGID: c_uint = 0x80084502;
    const EVIOCGNAME_BASE: c_uint = 0x80000000 | (b'E' as c_uint) << 8 | 0x06;
    const EVIOCGPHYS_BASE: c_uint = 0x80000000 | (b'E' as c_uint) << 8 | 0x07;
    const EVIOCGUNIQ_BASE: c_uint = 0x80000000 | (b'E' as c_uint) << 8 | 0x08;
    const EVIOCGPROP_BASE: c_uint = 0x80000000 | (b'E' as c_uint) << 8 | 0x09;
    const EVIOCGBIT_BASE: c_uint = 0x80000000 | (b'E' as c_uint) << 8 | 0x20;
    const EVIOCGABS_BASE: c_uint = 0x80000000 | (b'E' as c_uint) << 8 | 0x40;

    let request_nr = request & 0xFF;
    let request_type = (request >> 8) & 0xFF;

    match request {
        EVIOCGVERSION => {
            // Return input subsystem version
            let ptr: *mut c_int = unsafe { args.arg() };
            if !ptr.is_null() {
                unsafe {
                    *ptr = 0x010001;
                } // Version 1.0.1
                debug!("ioctl EVIOCGVERSION: returning 0x010001");
            }
            0
        }

        EVIOCGID => {
            // Return device ID (vendor, product, etc.)
            #[repr(C)]
            struct InputId {
                bustype: u16,
                vendor: u16,
                product: u16,
                version: u16,
            }

            let ptr: *mut InputId = unsafe { args.arg() };
            if !ptr.is_null() {
                // TODO: Get real device info from manager
                // For now, return Xbox 360 controller IDs
                unsafe {
                    *ptr = InputId {
                        bustype: 0x03, // USB
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
            // EVIOCGNAME - Get device name
            let ptr: *mut u8 = unsafe { args.arg() };
            let len = ((request >> 16) & 0x1FFF) as usize;

            if !ptr.is_null() && len > 0 {
                let name = b"Virtual Controller\0";
                let copy_len = std::cmp::min(name.len(), len);
                unsafe {
                    std::ptr::copy_nonoverlapping(name.as_ptr(), ptr, copy_len);
                }
                debug!("ioctl EVIOCGNAME: returning 'Virtual Controller'");
                copy_len as c_int
            } else {
                -1
            }
        }

        _ if request_type == b'E' as u32 && request_nr == 0x07 => {
            // EVIOCGPHYS - Get device physical location
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
            // EVIOCGUNIQ - Get device unique identifier
            let ptr: *mut u8 = unsafe { args.arg() };
            let len = ((request >> 16) & 0x1FFF) as usize;

            if !ptr.is_null() && len > 0 {
                // Empty unique ID
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
            // EVIOCGPROP - Get device properties
            let ptr: *mut u8 = unsafe { args.arg() };
            let len = ((request >> 16) & 0x1FFF) as usize;

            if !ptr.is_null() && len > 0 {
                // Zero out properties
                unsafe { std::ptr::write_bytes(ptr, 0, len) };
                debug!("ioctl EVIOCGPROP: returning empty properties");
                0
            } else {
                -1
            }
        }

        _ if request_type == b'E' as u32 && request_nr >= 0x20 && request_nr < 0x40 => {
            // EVIOCGBIT - Get event bits
            let ev_type = request_nr - 0x20;
            let ptr: *mut u8 = unsafe { args.arg() };
            let len = ((request >> 16) & 0x1FFF) as usize;

            if !ptr.is_null() && len > 0 {
                // Zero out first
                unsafe {
                    std::ptr::write_bytes(ptr, 0, len);
                }

                match ev_type {
                    0 => {
                        // EV_SYN, EV_KEY, EV_ABS are supported
                        if len > 0 {
                            unsafe {
                                *ptr = 0b00001011;
                            } // bits 0, 1, 3
                        }
                        debug!("ioctl EVIOCGBIT(0): returning supported event types");
                    }
                    1 => {
                        // EV_KEY - buttons
                        // Set bits for common gamepad buttons (BTN_SOUTH=304 to BTN_THUMBR=318)
                        if len >= 40 {
                            let _buttons_start = 304 / 8;
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
                        // EV_ABS - absolute axes
                        // Set bits for standard axes (0-5)
                        if len > 0 {
                            unsafe {
                                *ptr = 0b00111111;
                            } // bits 0-5
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
            // EVIOCGABS - Get absolute axis info
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
                // Return standard axis ranges
                unsafe {
                    *ptr = match axis {
                        0..=1 => InputAbsinfo {
                            // Left stick
                            value: 0,
                            minimum: -32768,
                            maximum: 32767,
                            fuzz: 16,
                            flat: 128,
                            resolution: 0,
                        },
                        2..=5 => InputAbsinfo {
                            // Triggers and right stick
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
            // Unknown ioctl, return error
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
