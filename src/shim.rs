use crate::protocol::{DeviceCommand, DeviceResponse, Message, Response};
use libc::{c_char, c_int, c_uint, c_void};
use libloading::Library;
use std::collections::HashMap;
use std::ffi::CStr;
use std::os::raw::c_short;
use std::ptr;
use std::sync::Mutex;
use tokio::io::{AsyncReadExt, AsyncWriteExt};
use tokio::net::UnixStream;
use tokio::sync::mpsc;
use ulid::Ulid;

// Type definitions for libevdev functions
type LibevdevNewFn = unsafe extern "C" fn() -> *mut c_void;
type LibevdevSetNameFn = unsafe extern "C" fn(dev: *mut c_void, name: *const c_char) -> c_int;
type LibevdevSetPhysFn = unsafe extern "C" fn(dev: *mut c_void, phys: *const c_char) -> c_int;
type LibevdevSetUniqFn = unsafe extern "C" fn(dev: *mut c_void, uniq: *const c_char) -> c_int;
type LibevdevSetIdBustypeFn = unsafe extern "C" fn(dev: *mut c_void, bustype: c_short) -> c_int;
type LibevdevSetIdVendorFn = unsafe extern "C" fn(dev: *mut c_void, vendor: c_short) -> c_int;
type LibevdevSetIdProductFn = unsafe extern "C" fn(dev: *mut c_void, product: c_short) -> c_int;
type LibevdevSetIdVersionFn = unsafe extern "C" fn(dev: *mut c_void, version: c_short) -> c_int;
type LibevdevSetDriverVersionFn = unsafe extern "C" fn(dev: *mut c_void, version: c_uint) -> c_int;
type LibevdevEnableEventTypeFn = unsafe extern "C" fn(dev: *mut c_void, type_: c_uint) -> c_int;
type LibevdevEnableEventCodeFn = unsafe extern "C" fn(
    dev: *mut c_void,
    type_: c_uint,
    code: c_uint,
    data: *const c_void,
) -> c_int;
type LibevdevUinputCreateFromDeviceFn =
    unsafe extern "C" fn(dev: *const c_void, flags: c_int, uinput_dev: *mut *mut c_void) -> c_int;
type LibevdevFreeFn = unsafe extern "C" fn(dev: *mut c_void);
type LibevdevUinputDestroyFn = unsafe extern "C" fn(uinput_dev: *mut c_void);
type LibevdevUinputWriteEventFn = unsafe extern "C" fn(
    uinput_dev: *mut c_void,
    type_: c_uint,
    code: c_uint,
    value: c_int,
) -> c_int;
type LibevdevUinputGetFdFn = unsafe extern "C" fn(uinput_dev: *mut c_void) -> c_int;
type LibevdevUinputGetDevnodeFn = unsafe extern "C" fn(uinput_dev: *mut c_void) -> *const c_char;
type LibevdevUinputGetSyspathFn = unsafe extern "C" fn(uinput_dev: *mut c_void) -> *const c_char;

// Global state for the shim
lazy_static::lazy_static! {
    static ref LIBEVDEV: Mutex<Option<Library>> = Mutex::new(None);
    static ref SOCKET_PATH: Mutex<Option<String>> = Mutex::new(None);
    static ref DEVICE_PTRS: Mutex<HashMap<u64, usize>> = Mutex::new(HashMap::new());
    static ref UINPUT_PTRS: Mutex<HashMap<u64, usize>> = Mutex::new(HashMap::new());
    static ref VIRTUAL_DEVICE_FDS: Mutex<HashMap<u64, c_int>> = Mutex::new(HashMap::new());
    static ref VIRTUAL_DEVICE_WRITE_FDS: Mutex<HashMap<u64, c_int>> = Mutex::new(HashMap::new());
    static ref VIRTUAL_DEVICE_NODES: Mutex<HashMap<u64, String>> = Mutex::new(HashMap::new());
    static ref VIRTUAL_DEVICE_SYSPATHS: Mutex<HashMap<u64, String>> = Mutex::new(HashMap::new());
    static ref RESPONSE_WAITERS: Mutex<HashMap<String, mpsc::UnboundedSender<DeviceResponse>>> = Mutex::new(HashMap::new());
}

// Initialize the shim
pub fn init_shim(socket_path: Option<String>) {
    *SOCKET_PATH.lock().unwrap() = socket_path;

    tracing::info!(
        "Initializing vimputti shim, socket path: {:?}",
        SOCKET_PATH.lock().unwrap()
    );

    // Load the real libevdev library
    unsafe {
        match Library::new("libevdev.so.2") {
            Ok(lib) => {
                *LIBEVDEV.lock().unwrap() = Some(lib);
            }
            Err(e) => {
                tracing::error!("Failed to load libevdev: {}", e);
            }
        }
    }
}

// Send a command to the manager and wait for a response
async fn send_command(command: DeviceCommand) -> Result<DeviceResponse, String> {
    let socket_path = SOCKET_PATH.lock().unwrap().clone();
    let socket_path = match socket_path {
        Some(path) => path,
        None => return Err("Socket path not set".to_string()),
    };

    let id = Ulid::new().to_string();
    let message = Message {
        id: id.clone(),
        command,
    };

    let message_json = serde_json::to_string(&message).map_err(|e| e.to_string())?;

    tracing::info!("Sending message: {}", message_json);

    // Connect to the manager socket
    let mut stream = UnixStream::connect(&socket_path)
        .await
        .map_err(|e| e.to_string())?;

    // Send the message
    stream
        .write_all(message_json.as_bytes())
        .await
        .map_err(|e| e.to_string())?;
    stream.write_u8(b'\n').await.map_err(|e| e.to_string())?;

    // Create a channel for the response
    let (tx, mut rx) = mpsc::unbounded_channel();
    RESPONSE_WAITERS.lock().unwrap().insert(id.clone(), tx);

    // Handle the response in the same connection
    let mut buffer = [0; 4096];
    let mut data = Vec::new();

    // Read the response
    match stream.read(&mut buffer).await {
        Ok(0) => return Err("Connection closed".to_string()),
        Ok(n) => {
            data.extend_from_slice(&buffer[..n]);

            // Process complete messages
            while let Some(pos) = data.iter().position(|&b| b == b'\n') {
                let message_data = data.drain(..=pos).collect::<Vec<_>>();
                let message_str = String::from_utf8_lossy(&message_data);

                tracing::info!("Received message: {}", message_str);

                if let Ok(response) = serde_json::from_str::<Response>(&message_str) {
                    if response.id == id {
                        return Ok(response.response);
                    }
                }
            }
        }
        Err(e) => return Err(format!("Error reading from socket: {}", e)),
    }

    // Wait for the response
    let response = rx.recv().await.ok_or("No response received")?;

    Ok(response)
}

// Get a symbol from the loaded libevdev library
fn get_libevdev_symbol<T>(symbol_name: &str) -> Result<T, String>
where
    T: Copy,
{
    let lib = LIBEVDEV.lock().unwrap();
    match lib.as_ref() {
        Some(lib) => unsafe {
            match lib.get::<T>(symbol_name.as_bytes()) {
                Ok(symbol) => Ok(*symbol),
                Err(e) => {
                    tracing::error!("Failed to get symbol {}: {}", symbol_name, e);
                    Err(format!("Failed to get symbol {}: {}", symbol_name, e))
                }
            }
        },
        None => Err("libevdev not loaded".to_string()),
    }
}

// Intercept libevdev_new
#[unsafe(no_mangle)]
pub unsafe extern "C" fn libevdev_new() -> *mut c_void {
    // Generate a unique pointer for this device
    let ptr = (DEVICE_PTRS.lock().unwrap().len() + 1) as u64;

    // Send the command to the manager
    let rt = tokio::runtime::Runtime::new().unwrap();
    match rt.block_on(send_command(DeviceCommand::New { ptr })) {
        Ok(DeviceResponse::Success) => {
            // Store the pointer
            DEVICE_PTRS.lock().unwrap().insert(ptr, ptr as usize);
            ptr as *mut c_void
        }
        _ => {
            // Fall back to the real libevdev if available
            if let Ok(libevdev_new) = get_libevdev_symbol::<LibevdevNewFn>("libevdev_new") {
                unsafe { libevdev_new() }
            } else {
                ptr::null_mut()
            }
        }
    }
}

// Intercept libevdev_set_name
#[unsafe(no_mangle)]
pub unsafe extern "C" fn libevdev_set_name(dev: *mut c_void, name: *const c_char) -> c_int {
    let ptr = dev as u64;
    let name_str = if name.is_null() {
        String::new()
    } else {
        unsafe { CStr::from_ptr(name).to_string_lossy().into_owned() }
    };

    // Send the command to the manager
    let rt = tokio::runtime::Runtime::new().unwrap();
    match rt.block_on(send_command(DeviceCommand::SetName {
        ptr,
        name: name_str,
    })) {
        Ok(DeviceResponse::Success) => 0,
        _ => {
            // Fall back to the real libevdev if available
            if let Ok(libevdev_set_name) =
                get_libevdev_symbol::<LibevdevSetNameFn>("libevdev_set_name")
            {
                unsafe { libevdev_set_name(dev, name) }
            } else {
                -1
            }
        }
    }
}

// Intercept libevdev_set_phys
#[unsafe(no_mangle)]
pub unsafe extern "C" fn libevdev_set_phys(dev: *mut c_void, phys: *const c_char) -> c_int {
    let ptr = dev as u64;
    let phys_str = if phys.is_null() {
        String::new()
    } else {
        unsafe { CStr::from_ptr(phys).to_string_lossy().into_owned() }
    };

    // Send the command to the manager
    let rt = tokio::runtime::Runtime::new().unwrap();
    match rt.block_on(send_command(DeviceCommand::SetPhys {
        ptr,
        phys: phys_str,
    })) {
        Ok(DeviceResponse::Success) => 0,
        _ => {
            // Fall back to the real libevdev if available
            if let Ok(libevdev_set_phys) =
                get_libevdev_symbol::<LibevdevSetPhysFn>("libevdev_set_phys")
            {
                unsafe { libevdev_set_phys(dev, phys) }
            } else {
                -1
            }
        }
    }
}

// Intercept libevdev_set_uniq
#[unsafe(no_mangle)]
pub unsafe extern "C" fn libevdev_set_uniq(dev: *mut c_void, uniq: *const c_char) -> c_int {
    let ptr = dev as u64;
    let uniq_str = if uniq.is_null() {
        String::new()
    } else {
        unsafe { CStr::from_ptr(uniq).to_string_lossy().into_owned() }
    };

    // Send the command to the manager
    let rt = tokio::runtime::Runtime::new().unwrap();
    match rt.block_on(send_command(DeviceCommand::SetUniq {
        ptr,
        uniq: uniq_str,
    })) {
        Ok(DeviceResponse::Success) => 0,
        _ => {
            // Fall back to the real libevdev if available
            if let Ok(libevdev_set_uniq) =
                get_libevdev_symbol::<LibevdevSetUniqFn>("libevdev_set_uniq")
            {
                unsafe { libevdev_set_uniq(dev, uniq) }
            } else {
                -1
            }
        }
    }
}

// Intercept libevdev_set_id_bustype
#[unsafe(no_mangle)]
pub unsafe extern "C" fn libevdev_set_id_bustype(dev: *mut c_void, bustype: c_short) -> c_int {
    let ptr = dev as u64;

    // Send the command to the manager
    let rt = tokio::runtime::Runtime::new().unwrap();
    match rt.block_on(send_command(DeviceCommand::SetIdBustype {
        ptr,
        bustype: bustype as u16,
    })) {
        Ok(DeviceResponse::Success) => 0,
        _ => {
            // Fall back to the real libevdev if available
            if let Ok(libevdev_set_id_bustype) =
                get_libevdev_symbol::<LibevdevSetIdBustypeFn>("libevdev_set_id_bustype")
            {
                unsafe { libevdev_set_id_bustype(dev, bustype) }
            } else {
                -1
            }
        }
    }
}

// Intercept libevdev_set_id_vendor
#[unsafe(no_mangle)]
pub unsafe extern "C" fn libevdev_set_id_vendor(dev: *mut c_void, vendor: c_short) -> c_int {
    let ptr = dev as u64;

    // Send the command to the manager
    let rt = tokio::runtime::Runtime::new().unwrap();
    match rt.block_on(send_command(DeviceCommand::SetIdVendor {
        ptr,
        vendor: vendor as u16,
    })) {
        Ok(DeviceResponse::Success) => 0,
        _ => {
            // Fall back to the real libevdev if available
            if let Ok(libevdev_set_id_vendor) =
                get_libevdev_symbol::<LibevdevSetIdVendorFn>("libevdev_set_id_vendor")
            {
                unsafe { libevdev_set_id_vendor(dev, vendor) }
            } else {
                -1
            }
        }
    }
}

// Intercept libevdev_set_id_product
#[unsafe(no_mangle)]
pub unsafe extern "C" fn libevdev_set_id_product(dev: *mut c_void, product: c_short) -> c_int {
    let ptr = dev as u64;

    // Send the command to the manager
    let rt = tokio::runtime::Runtime::new().unwrap();
    match rt.block_on(send_command(DeviceCommand::SetIdProduct {
        ptr,
        product: product as u16,
    })) {
        Ok(DeviceResponse::Success) => 0,
        _ => {
            // Fall back to the real libevdev if available
            if let Ok(libevdev_set_id_product) =
                get_libevdev_symbol::<LibevdevSetIdProductFn>("libevdev_set_id_product")
            {
                unsafe { libevdev_set_id_product(dev, product) }
            } else {
                -1
            }
        }
    }
}

// Intercept libevdev_set_id_version
#[unsafe(no_mangle)]
pub unsafe extern "C" fn libevdev_set_id_version(dev: *mut c_void, version: c_short) -> c_int {
    let ptr = dev as u64;

    // Send the command to the manager
    let rt = tokio::runtime::Runtime::new().unwrap();
    match rt.block_on(send_command(DeviceCommand::SetIdVersion {
        ptr,
        version: version as u16,
    })) {
        Ok(DeviceResponse::Success) => 0,
        _ => {
            // Fall back to the real libevdev if available
            if let Ok(libevdev_set_id_version) =
                get_libevdev_symbol::<LibevdevSetIdVersionFn>("libevdev_set_id_version")
            {
                unsafe { libevdev_set_id_version(dev, version) }
            } else {
                -1
            }
        }
    }
}

// Intercept libevdev_set_driver_version
#[unsafe(no_mangle)]
pub unsafe extern "C" fn libevdev_set_driver_version(dev: *mut c_void, version: c_uint) -> c_int {
    let ptr = dev as u64;

    // Send the command to the manager
    let rt = tokio::runtime::Runtime::new().unwrap();
    match rt.block_on(send_command(DeviceCommand::SetDriverVersion {
        ptr,
        version,
    })) {
        Ok(DeviceResponse::Success) => 0,
        _ => {
            // Fall back to the real libevdev if available
            if let Ok(libevdev_set_driver_version) =
                get_libevdev_symbol::<LibevdevSetDriverVersionFn>("libevdev_set_driver_version")
            {
                unsafe { libevdev_set_driver_version(dev, version) }
            } else {
                -1
            }
        }
    }
}

// Intercept libevdev_enable_event_type
#[unsafe(no_mangle)]
pub unsafe extern "C" fn libevdev_enable_event_type(dev: *mut c_void, type_: c_uint) -> c_int {
    let ptr = dev as u64;

    // Send the command to the manager
    let rt = tokio::runtime::Runtime::new().unwrap();
    match rt.block_on(send_command(DeviceCommand::EnableEventType { ptr, type_ })) {
        Ok(DeviceResponse::Success) => 0,
        _ => {
            // Fall back to the real libevdev if available
            if let Ok(libevdev_enable_event_type) =
                get_libevdev_symbol::<LibevdevEnableEventTypeFn>("libevdev_enable_event_type")
            {
                unsafe { libevdev_enable_event_type(dev, type_) }
            } else {
                -1
            }
        }
    }
}

// Intercept libevdev_enable_event_code
#[unsafe(no_mangle)]
pub unsafe extern "C" fn libevdev_enable_event_code(
    dev: *mut c_void,
    type_: c_uint,
    code: c_uint,
    data: *const c_void,
) -> c_int {
    let ptr = dev as u64;

    // Send the command to the manager
    let rt = tokio::runtime::Runtime::new().unwrap();
    match rt.block_on(send_command(DeviceCommand::EnableEventCode {
        ptr,
        type_,
        code,
    })) {
        Ok(DeviceResponse::Success) => 0,
        _ => {
            // Fall back to the real libevdev if available
            if let Ok(libevdev_enable_event_code) =
                get_libevdev_symbol::<LibevdevEnableEventCodeFn>("libevdev_enable_event_code")
            {
                unsafe { libevdev_enable_event_code(dev, type_, code, data) }
            } else {
                -1
            }
        }
    }
}

// Intercept libevdev_uinput_create_from_device
#[unsafe(no_mangle)]
pub unsafe extern "C" fn libevdev_uinput_create_from_device(
    dev: *const c_void,
    flags: c_int,
    uinput_dev: *mut *mut c_void,
) -> c_int {
    let ptr = dev as u64;
    let uinput_ptr = (UINPUT_PTRS.lock().unwrap().len() + 1) as u64;

    // Send the command to the manager
    let rt = tokio::runtime::Runtime::new().unwrap();
    match rt.block_on(send_command(DeviceCommand::UinputCreateFromDevice {
        ptr,
        uinput_ptr,
    })) {
        Ok(DeviceResponse::UinputCreated { uinput_ptr: _ }) => {
            // Store the uinput pointer
            UINPUT_PTRS
                .lock()
                .unwrap()
                .insert(uinput_ptr, uinput_ptr as usize);
            unsafe {
                *uinput_dev = uinput_ptr as *mut c_void;
            }
            0
        }
        _ => {
            // Fall back to the real libevdev if available
            if let Ok(libevdev_uinput_create_from_device) =
                get_libevdev_symbol::<LibevdevUinputCreateFromDeviceFn>(
                    "libevdev_uinput_create_from_device",
                )
            {
                unsafe { libevdev_uinput_create_from_device(dev, flags, uinput_dev) }
            } else {
                -1
            }
        }
    }
}

// Intercept libevdev_free
#[unsafe(no_mangle)]
pub unsafe extern "C" fn libevdev_free(dev: *mut c_void) {
    let ptr = dev as u64;

    // Check if this is a virtual device (in our map)
    let is_virtual = DEVICE_PTRS.lock().unwrap().contains_key(&ptr);

    // Send the command to the manager
    let rt = tokio::runtime::Runtime::new().unwrap();
    let _ = rt.block_on(send_command(DeviceCommand::Free { ptr }));

    // Remove the pointer from our map
    DEVICE_PTRS.lock().unwrap().remove(&ptr);

    // Only call the real libevdev_free if this is not a virtual device
    if !is_virtual {
        if let Ok(libevdev_free) = get_libevdev_symbol::<LibevdevFreeFn>("libevdev_free") {
            unsafe {
                libevdev_free(dev);
            }
        }
    }
}

// Intercept libevdev_uinput_destroy
#[unsafe(no_mangle)]
pub unsafe extern "C" fn libevdev_uinput_destroy(uinput_dev: *mut c_void) {
    let uinput_ptr = uinput_dev as u64;

    // Check if this is a virtual uinput device (in our map)
    let is_virtual = UINPUT_PTRS.lock().unwrap().contains_key(&uinput_ptr);

    // Send the command to the manager
    let rt = tokio::runtime::Runtime::new().unwrap();
    let _ = rt.block_on(send_command(DeviceCommand::UinputDestroy { uinput_ptr }));

    // Clean up our resources
    if is_virtual {
        // Close and remove the read file descriptor
        if let Some(fd) = VIRTUAL_DEVICE_FDS.lock().unwrap().remove(&uinput_ptr) {
            unsafe {
                libc::close(fd);
            }
        }

        // Close and remove the write file descriptor
        if let Some(fd) = VIRTUAL_DEVICE_WRITE_FDS.lock().unwrap().remove(&uinput_ptr) {
            unsafe {
                libc::close(fd);
            }
        }

        // Remove the device node and syspath
        VIRTUAL_DEVICE_NODES.lock().unwrap().remove(&uinput_ptr);
        VIRTUAL_DEVICE_SYSPATHS.lock().unwrap().remove(&uinput_ptr);
    }

    // Remove the uinput pointer from our map
    UINPUT_PTRS.lock().unwrap().remove(&uinput_ptr);

    // Only call the real libevdev_uinput_destroy if this is not a virtual device
    if !is_virtual {
        if let Ok(libevdev_uinput_destroy) =
            get_libevdev_symbol::<LibevdevUinputDestroyFn>("libevdev_uinput_destroy")
        {
            unsafe {
                libevdev_uinput_destroy(uinput_dev);
            }
        }
    }
}

// Intercept libevdev_uinput_write_event
#[unsafe(no_mangle)]
pub unsafe extern "C" fn libevdev_uinput_write_event(
    uinput_dev: *mut c_void,
    type_: c_uint,
    code: c_uint,
    value: c_int,
) -> c_int {
    let uinput_ptr = uinput_dev as u64;

    // Check if this is a virtual uinput device (in our map)
    let is_virtual = UINPUT_PTRS.lock().unwrap().contains_key(&uinput_ptr);

    if is_virtual {
        // Send the command to the manager
        let rt = tokio::runtime::Runtime::new().unwrap();
        match rt.block_on(send_command(DeviceCommand::UinputWriteEvent {
            uinput_ptr,
            type_,
            code,
            value,
        })) {
            Ok(DeviceResponse::Success) => {
                // For now, we just return success
                // In a complete implementation, you would write the event to the pipe
                // so that applications can read it from the file descriptor
                0
            }
            _ => -1,
        }
    } else {
        // Fall back to the real libevdev if available
        if let Ok(libevdev_uinput_write_event) =
            get_libevdev_symbol::<LibevdevUinputWriteEventFn>("libevdev_uinput_write_event")
        {
            unsafe { libevdev_uinput_write_event(uinput_dev, type_, code, value) }
        } else {
            -1
        }
    }
}

// Intercept libevdev_uinput_get_fd
#[unsafe(no_mangle)]
pub unsafe extern "C" fn libevdev_uinput_get_fd(uinput_dev: *mut c_void) -> c_int {
    let uinput_ptr = uinput_dev as u64;

    // Check if this is a virtual uinput device (in our map)
    let is_virtual = UINPUT_PTRS.lock().unwrap().contains_key(&uinput_ptr);

    if is_virtual {
        // Return the real file descriptor for this virtual device
        if let Some(fd) = VIRTUAL_DEVICE_FDS.lock().unwrap().get(&uinput_ptr) {
            return *fd;
        }

        // Create a new pipe if it doesn't exist
        let mut fds = [0i32; 2];
        if unsafe { libc::pipe(fds.as_mut_ptr()) } == -1 {
            return -1;
        }

        // Set the read end to non-blocking
        let flags = unsafe { libc::fcntl(fds[0], libc::F_GETFL) };
        if flags == -1 {
            unsafe {
                libc::close(fds[0]);
            }
            unsafe {
                libc::close(fds[1]);
            }
            return -1;
        }

        if unsafe { libc::fcntl(fds[0], libc::F_SETFL, flags | libc::O_NONBLOCK) } == -1 {
            unsafe {
                libc::close(fds[0]);
            }
            unsafe {
                libc::close(fds[1]);
            }
            return -1;
        }

        // Store the read end for the device
        VIRTUAL_DEVICE_FDS
            .lock()
            .unwrap()
            .insert(uinput_ptr, fds[0]);

        // Store the write end for event injection (we'll use this later)
        // For now, we can close the write end or store it for later use
        // Let's store it in a separate map for now
        VIRTUAL_DEVICE_WRITE_FDS
            .lock()
            .unwrap()
            .insert(uinput_ptr, fds[1]);

        fds[0]
    } else {
        // Fall back to the real libevdev if available
        if let Ok(libevdev_uinput_get_fd) =
            get_libevdev_symbol::<LibevdevUinputGetFdFn>("libevdev_uinput_get_fd")
        {
            unsafe { libevdev_uinput_get_fd(uinput_dev) }
        } else {
            -1
        }
    }
}

// Intercept libevdev_uinput_get_devnode
#[unsafe(no_mangle)]
pub unsafe extern "C" fn libevdev_uinput_get_devnode(uinput_dev: *mut c_void) -> *const c_char {
    let uinput_ptr = uinput_dev as u64;

    // Check if this is a virtual uinput device (in our map)
    let is_virtual = UINPUT_PTRS.lock().unwrap().contains_key(&uinput_ptr);

    if is_virtual {
        // Check if we already have a device node for this virtual device
        if let Some(node) = VIRTUAL_DEVICE_NODES.lock().unwrap().get(&uinput_ptr) {
            return std::ffi::CString::new(node.clone()).unwrap().into_raw();
        }

        // Create a virtual device node path
        let node = format!("/dev/input/vimputti-{}", uinput_ptr);
        VIRTUAL_DEVICE_NODES
            .lock()
            .unwrap()
            .insert(uinput_ptr, node.clone());

        std::ffi::CString::new(node).unwrap().into_raw()
    } else {
        // Fall back to the real libevdev if available
        if let Ok(libevdev_uinput_get_devnode) =
            get_libevdev_symbol::<LibevdevUinputGetDevnodeFn>("libevdev_uinput_get_devnode")
        {
            unsafe { libevdev_uinput_get_devnode(uinput_dev) }
        } else {
            ptr::null()
        }
    }
}

// Intercept libevdev_uinput_get_syspath
#[unsafe(no_mangle)]
pub unsafe extern "C" fn libevdev_uinput_get_syspath(uinput_dev: *mut c_void) -> *const c_char {
    let uinput_ptr = uinput_dev as u64;

    // Check if this is a virtual uinput device (in our map)
    let is_virtual = UINPUT_PTRS.lock().unwrap().contains_key(&uinput_ptr);

    if is_virtual {
        // Check if we already have a syspath for this virtual device
        if let Some(syspath) = VIRTUAL_DEVICE_SYSPATHS.lock().unwrap().get(&uinput_ptr) {
            return std::ffi::CString::new(syspath.clone()).unwrap().into_raw();
        }

        // Create a virtual sysfs path
        let syspath = format!("/sys/devices/virtual/input/vimputti-{}", uinput_ptr);
        VIRTUAL_DEVICE_SYSPATHS
            .lock()
            .unwrap()
            .insert(uinput_ptr, syspath.clone());

        std::ffi::CString::new(syspath).unwrap().into_raw()
    } else {
        // Fall back to the real libevdev if available
        if let Ok(libevdev_uinput_get_syspath) =
            get_libevdev_symbol::<LibevdevUinputGetSyspathFn>("libevdev_uinput_get_syspath")
        {
            unsafe { libevdev_uinput_get_syspath(uinput_dev) }
        } else {
            ptr::null()
        }
    }
}

// Initialize the shim when the library is loaded
#[ctor::ctor]
fn init() {
    // Initialize tracing subscriber for logging
    tracing_subscriber::fmt()
        .with_env_filter(
            tracing_subscriber::EnvFilter::builder()
                .with_default_directive(tracing_subscriber::filter::LevelFilter::INFO.into())
                .from_env()
                .unwrap(),
        )
        .init();

    // Set up signal handler for SIGSEGV
    unsafe {
        libc::signal(libc::SIGSEGV, std::mem::transmute(sigsegv_handler as usize));
    }

    // Get the socket path from environment variable or use default
    let socket_path = match std::env::var("VIMPUTTI_SOCKET_PATH") {
        Ok(path) => Some(path),
        Err(_) => {
            // Use default path
            let uid = unsafe { libc::getuid() };
            Some(format!("/run/user/{}/vimputti-0", uid))
        }
    };

    init_shim(socket_path);
}

extern "C" fn sigsegv_handler(sig: c_int) {
    tracing::error!("Caught SIGSEGV! Signal: {}", sig);
    tracing::error!("Backtrace:");
    tracing::error!("{:?}", backtrace::Backtrace::new());
    unsafe {
        libc::_exit(1);
    }
}
