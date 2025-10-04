use serde::{Deserialize, Serialize};

/// Unique identifier for a virtual device
pub type DeviceId = u64;

/// Message sent from library client to manager
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ControlMessage {
    pub id: String, // ULID for request/response matching
    pub command: ControlCommand,
}

/// Response sent from manager to library client
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ControlResponse {
    pub id: String, // Matches request ID
    pub result: ControlResult,
}

/// Commands that can be sent to the manager
#[derive(Debug, Clone, Serialize, Deserialize)]
pub enum ControlCommand {
    /// Create a new virtual device
    CreateDevice { config: DeviceConfig },
    /// Destroy a virtual device (explicit, though drop also works)
    DestroyDevice { device_id: DeviceId },
    /// Send input events to a device
    SendInput {
        device_id: DeviceId,
        events: Vec<InputEvent>,
    },
    /// Query all active devices
    ListDevices,
    /// Ping to check if manager is alive
    Ping,
}

/// Results returned by the manager
#[derive(Debug, Clone, Serialize, Deserialize)]
pub enum ControlResult {
    /// Device successfully created
    DeviceCreated {
        device_id: DeviceId,
        event_node: String, // e.g., "event0"
    },
    /// Device successfully destroyed
    DeviceDestroyed,
    /// Input events successfully sent
    InputSent,
    /// List of active devices
    DeviceList(Vec<DeviceInfo>),
    /// Pong response
    Pong,
    /// Error occurred
    Error { message: String },
}

/// Configuration for creating a virtual device
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct DeviceConfig {
    pub name: String,
    pub vendor_id: u16,
    pub product_id: u16,
    pub version: u16,
    pub bustype: BusType,
    pub buttons: Vec<Button>,
    pub axes: Vec<AxisConfig>,
}

/// Bus type for input devices
#[derive(Debug, Clone, Copy, Serialize, Deserialize)]
pub enum BusType {
    Usb = 0x03,
    Bluetooth = 0x05,
    Virtual = 0x06,
}

/// Common controller buttons
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub enum Button {
    // Face buttons
    A,
    B,
    X,
    Y,

    // Shoulder buttons
    LeftBumper,
    RightBumper,
    LeftTrigger,
    RightTrigger,

    // Stick buttons
    LeftStick,
    RightStick,

    // D-pad
    DPadUp,
    DPadDown,
    DPadLeft,
    DPadRight,

    // Special buttons
    Start,
    Select,
    Guide,

    // Custom button with raw code
    Custom(u16),
}

impl Button {
    /// Convert button to Linux input event code
    pub fn to_code(self) -> u16 {
        match self {
            Button::A => 304,            // BTN_SOUTH
            Button::B => 305,            // BTN_EAST
            Button::X => 307,            // BTN_NORTH
            Button::Y => 308,            // BTN_WEST
            Button::LeftBumper => 310,   // BTN_TL
            Button::RightBumper => 311,  // BTN_TR
            Button::LeftTrigger => 312,  // BTN_TL2
            Button::RightTrigger => 313, // BTN_TR2
            Button::LeftStick => 317,    // BTN_THUMBL
            Button::RightStick => 318,   // BTN_THUMBR
            Button::Start => 315,        // BTN_START
            Button::Select => 314,       // BTN_SELECT
            Button::Guide => 316,        // BTN_MODE
            Button::DPadUp => 544,       // BTN_DPAD_UP
            Button::DPadDown => 545,     // BTN_DPAD_DOWN
            Button::DPadLeft => 546,     // BTN_DPAD_LEFT
            Button::DPadRight => 547,    // BTN_DPAD_RIGHT
            Button::Custom(code) => code,
        }
    }

    /// Create a button from a zero-based index (for bitmasking)
    pub fn from_index(index: u8) -> Self {
        match index {
            0 => Button::A,
            1 => Button::B,
            2 => Button::X,
            3 => Button::Y,
            4 => Button::LeftBumper,
            5 => Button::RightBumper,
            6 => Button::LeftTrigger,
            7 => Button::RightTrigger,
            8 => Button::Select,
            9 => Button::Start,
            10 => Button::LeftStick,
            11 => Button::RightStick,
            12 => Button::DPadUp,
            13 => Button::DPadDown,
            14 => Button::DPadLeft,
            15 => Button::DPadRight,
            16 => Button::Guide,
            _ => Button::Custom(0x100 + index as u16), // Custom buttons start at code 0x100
        }
    }
}

/// Controller axis
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub enum Axis {
    LeftStickX,
    LeftStickY,
    RightStickX,
    RightStickY,
    LeftTrigger,
    RightTrigger,
    DPadX,
    DPadY,
    Custom(u16),
}

impl Axis {
    /// Convert axis to Linux input event code
    pub fn to_code(self) -> u16 {
        match self {
            Axis::LeftStickX => 0,   // ABS_X
            Axis::LeftStickY => 1,   // ABS_Y
            Axis::RightStickX => 3,  // ABS_RX
            Axis::RightStickY => 4,  // ABS_RY
            Axis::LeftTrigger => 2,  // ABS_Z
            Axis::RightTrigger => 5, // ABS_RZ
            Axis::DPadX => 16,       // ABS_HAT0X
            Axis::DPadY => 17,       // ABS_HAT0Y
            Axis::Custom(code) => code,
        }
    }
}

/// Configuration for an axis
#[derive(Debug, Clone, Copy, Serialize, Deserialize)]
pub struct AxisConfig {
    pub axis: Axis,
    pub min: i32,
    pub max: i32,
    pub fuzz: i32,
    pub flat: i32,
}

impl AxisConfig {
    pub fn new(axis: Axis, min: i32, max: i32) -> Self {
        Self {
            axis,
            min,
            max,
            fuzz: 0,
            flat: 0,
        }
    }
}

/// Input event to send to a device
#[derive(Debug, Clone, Serialize, Deserialize)]
pub enum InputEvent {
    /// Button press/release
    Button { button: Button, pressed: bool },
    /// Axis movement
    Axis { axis: Axis, value: i32 },
    /// Raw Linux input event
    Raw {
        event_type: u16,
        code: u16,
        value: i32,
    },
    /// Synchronization event (automatically added if not present)
    Sync,
}

/// Information about an active device
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct DeviceInfo {
    pub device_id: DeviceId,
    pub name: String,
    pub event_node: String,
    pub vendor_id: u16,
    pub product_id: u16,
}

/// Linux input event structure (for sending to device sockets)
#[derive(Debug, Clone, Copy)]
#[repr(C)]
pub struct LinuxInputEvent {
    pub time: TimeVal,
    pub event_type: u16,
    pub code: u16,
    pub value: i32,
}

#[derive(Debug, Clone, Copy)]
#[repr(C)]
pub struct TimeVal {
    pub tv_sec: i64,
    pub tv_usec: i64,
}

impl LinuxInputEvent {
    pub fn new(event_type: u16, code: u16, value: i32) -> Self {
        Self {
            time: TimeVal::now(),
            event_type,
            code,
            value,
        }
    }

    pub fn to_bytes(&self) -> [u8; 24] {
        unsafe { std::mem::transmute(*self) }
    }
}

impl TimeVal {
    pub fn now() -> Self {
        let now = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap();
        Self {
            tv_sec: now.as_secs() as i64,
            tv_usec: now.subsec_micros() as i64,
        }
    }
}

// Linux input event type constants
pub const EV_SYN: u16 = 0x00;
pub const EV_KEY: u16 = 0x01;
pub const EV_ABS: u16 = 0x03;

pub const SYN_REPORT: u16 = 0;
