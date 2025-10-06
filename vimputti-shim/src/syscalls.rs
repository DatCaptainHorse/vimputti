use libc::{c_int, c_uint};
use std::collections::HashMap;
use std::os::unix::io::RawFd;
use std::sync::Mutex;
use tracing::{debug, warn};
use vimputti::*;

lazy_static::lazy_static! {
    // Track which FDs are our virtual device sockets
    static ref VIRTUAL_DEVICE_FDS: Mutex<HashMap<RawFd, DeviceInfo>> = Mutex::new(HashMap::new());
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

/// Open a device node (actually connect to Unix socket)
pub fn open_device_node(socket_path: &str, _flags: c_int) -> c_int {
    use std::io::Read;
    use std::os::unix::io::IntoRawFd;
    use std::os::unix::net::UnixStream;

    debug!("Opening device node: {}", socket_path);

    match UnixStream::connect(socket_path) {
        Ok(mut stream) => {
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
                                warn!("Failed to deserialize device config: {}, using default", e);
                                vimputti::templates::ControllerTemplates::xbox360()
                            }
                        },
                        Err(e) => {
                            warn!("Failed to read device config data: {}, using default", e);
                            vimputti::templates::ControllerTemplates::xbox360()
                        }
                    }
                }
                Err(e) => {
                    warn!("Failed to read config length: {}, using default", e);
                    vimputti::templates::ControllerTemplates::xbox360()
                }
            };

            let fd = stream.into_raw_fd();

            // Register this FD as a virtual device
            VIRTUAL_DEVICE_FDS.lock().unwrap().insert(
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
            return -1;
            //return unsafe { handle_joystick_ioctl(fd, request, args, &info) };
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
                debug!("ioctl JSIOCGVERSION: returning 0x020100");
            }
            0
        }

        JSIOCGAXES => {
            let ptr: *mut u8 = unsafe { args.arg() };
            if !ptr.is_null() {
                unsafe {
                    *ptr = device_info.num_axes();
                }
                debug!(
                    "ioctl JSIOCGAXES: returning {} axes",
                    device_info.num_axes()
                );
            }
            0
        }

        JSIOCGBUTTONS => {
            let ptr: *mut u8 = unsafe { args.arg() };
            if !ptr.is_null() {
                unsafe {
                    *ptr = device_info.num_buttons();
                }
                debug!(
                    "ioctl JSIOCGBUTTONS: returning {} buttons",
                    device_info.num_buttons()
                );
            }
            0
        }

        JSIOCGAXMAP => {
            let ptr: *mut u8 = unsafe { args.arg() };
            let len = ((request >> 16) & 0x3FFF) as usize;

            if !ptr.is_null() && len > 0 {
                // Build axis map from device config
                let mut axis_map = Vec::new();
                for axis_config in &device_info.config.axes {
                    //axis_map.push(axis_config.axis.to_js_code() as u8);
                }

                let copy_len = std::cmp::min(axis_map.len(), len);
                unsafe {
                    std::ptr::copy_nonoverlapping(axis_map.as_ptr(), ptr, copy_len);
                }
                debug!(
                    "ioctl JSIOCGAXMAP: returning axis map with {} axes",
                    axis_map.len()
                );
            }
            0
        }

        JSIOCGBTNMAP => {
            let ptr: *mut u16 = unsafe { args.arg() };
            let len = ((request >> 16) & 0x3FFF) as usize / 2;

            if !ptr.is_null() && len > 0 {
                // Build button map from device config
                let mut button_map = Vec::new();
                for button in &device_info.config.buttons {
                    //button_map.push(button.to_code());
                }

                let copy_len = std::cmp::min(button_map.len(), len);
                unsafe {
                    std::ptr::copy_nonoverlapping(button_map.as_ptr(), ptr, copy_len);
                }
                debug!(
                    "ioctl JSIOCGBTNMAP: returning button map with {} buttons",
                    button_map.len()
                );
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
                debug!(
                    "ioctl JSIOCGNAME: returning '{}'",
                    device_info.device_name()
                );
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

/* Linux input EV reference:
#define EVIOCGNAME(len)		_IOC(_IOC_READ, 'E', 0x06, len)		/* get device name */
#define EVIOCGPHYS(len)		_IOC(_IOC_READ, 'E', 0x07, len)		/* get physical location */
#define EVIOCGUNIQ(len)		_IOC(_IOC_READ, 'E', 0x08, len)		/* get unique identifier */
#define EVIOCGPROP(len)		_IOC(_IOC_READ, 'E', 0x09, len)		/* get device properties */

#define EVIOCGKEY(len)		_IOC(_IOC_READ, 'E', 0x18, len)		/* get global key state */
#define EVIOCGLED(len)		_IOC(_IOC_READ, 'E', 0x19, len)		/* get all LEDs */
#define EVIOCGSND(len)		_IOC(_IOC_READ, 'E', 0x1a, len)		/* get all sounds status */
#define EVIOCGSW(len)		_IOC(_IOC_READ, 'E', 0x1b, len)		/* get all switch states */

#define EVIOCGBIT(ev,len)	_IOC(_IOC_READ, 'E', 0x20 + (ev), len)	/* get event bits */
#define EVIOCGABS(abs)		_IOR('E', 0x40 + (abs), struct input_absinfo)	/* get abs value/limits */
#define EVIOCSABS(abs)		_IOW('E', 0xc0 + (abs), struct input_absinfo)	/* set abs value/limits */
*/

/// Handle evdev interface ioctl calls
unsafe fn handle_evdev_ioctl(
    _fd: RawFd,
    request: c_uint,
    args: &mut std::ffi::VaListImpl,
    device_info: &DeviceInfo,
) -> c_int {
    const EVIOCGVERSION: c_uint = 0x80044501;
    const EVIOCGID: c_uint = 0x80084502;
    const EVIOCGNAME: c_uint = 0x81004506;
    const EVIOCGPHYS: c_uint = 0x81004507;
    const EVIOCGUNIQ: c_uint = 0x81004508;
    const EVIOCGPROP: c_uint = 0x81004509;

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
                        bustype: device_info.config.bustype as u16,
                        vendor: device_info.config.vendor_id,
                        product: device_info.config.product_id,
                        version: device_info.config.version,
                    };
                }
                debug!(
                    "ioctl EVIOCGID: returning vendor=0x{:04x}, product=0x{:04x}",
                    device_info.config.vendor_id, device_info.config.product_id
                );
            }
            0
        }

        EVIOCGNAME => {
            let ptr: *mut u8 = unsafe { args.arg() };
            let len = ((request >> 16) & 0x1FFF) as usize;

            if !ptr.is_null() && len > 0 {
                let name_bytes = device_info.device_name().as_bytes();
                let copy_len = std::cmp::min(name_bytes.len(), len - 1);
                unsafe {
                    std::ptr::copy_nonoverlapping(name_bytes.as_ptr(), ptr, copy_len);
                }
                unsafe {
                    *ptr.add(copy_len) = 0;
                }
                debug!(
                    "ioctl EVIOCGNAME: returning '{}'",
                    device_info.device_name()
                );
                copy_len as c_int
            } else {
                -1
            }
        }

        EVIOCGPHYS => {
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

        EVIOCGUNIQ => {
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

        EVIOCGPROP => {
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
                    EV_SYN => unsafe {
                        *ptr |= 1 << (0 % 8);
                    },
                    EV_KEY => {
                        for button in &device_info.config.buttons {
                            let code = button.to_ev_code() as usize;
                            unsafe {
                                *ptr.add(code / 8) |= 1 << (code % 8);
                            }
                        }
                    }
                    EV_ABS => {
                        for axis in &device_info.config.axes {
                            let code = axis.axis.to_ev_code() as usize;
                            unsafe {
                                *ptr.add(code / 8) |= 1 << (code % 8);
                            }
                        }
                        debug!("ioctl EVIOCGBIT(EV_ABS): returning axis bits");
                    }
                    _ => {
                        debug!("ioctl EVIOCGBIT({}): unknown type", ev_type);
                    }
                }
                len as c_int
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
                let default_info = LinuxAbsEvent {
                    value: 0,
                    minimum: -32768,
                    maximum: 32767,
                    fuzz: 16,
                    flat: 128,
                    resolution: 0,
                };

                unsafe {
                    *ptr = axis_info.unwrap_or(default_info);
                }
                debug!("ioctl EVIOCGABS({}): returning axis info", axis_code);
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
