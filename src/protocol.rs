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
    UpperLeftBumper,
    UpperRightBumper,
    LowerLeftTrigger,
    LowerRightTrigger,

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
    pub fn to_ev_code(self) -> u16 {
        match self {
            Button::A => 0x130,                 // BTN_SOUTH
            Button::B => 0x131,                 // BTN_EAST
            Button::X => 0x133,                 // BTN_NORTH
            Button::Y => 0x134,                 // BTN_WEST
            Button::UpperLeftBumper => 0x136,   // BTN_TL
            Button::UpperRightBumper => 0x137,  // BTN_TR
            Button::LowerLeftTrigger => 0x138,  // BTN_TL2
            Button::LowerRightTrigger => 0x139, // BTN_TR2
            Button::LeftStick => 0x13d,         // BTN_THUMBL
            Button::RightStick => 0x13e,        // BTN_THUMBR
            Button::Start => 0x13b,             // BTN_START
            Button::Select => 0x13a,            // BTN_SELECT
            Button::Guide => 0x13c,             // BTN_MODE
            Button::DPadUp => 0x220,            // BTN_DPAD_UP
            Button::DPadDown => 0x221,          // BTN_DPAD_DOWN
            Button::DPadLeft => 0x222,          // BTN_DPAD_LEFT
            Button::DPadRight => 0x223,         // BTN_DPAD_RIGHT
            Button::Custom(code) => code,
        }
    }

    /// Convert from Linux input event code to Button
    pub fn from_ev_code(code: u16) -> Option<Self> {
        match code {
            0x130 => Some(Button::A),
            0x131 => Some(Button::B),
            0x133 => Some(Button::X),
            0x134 => Some(Button::Y),
            0x136 => Some(Button::UpperLeftBumper),
            0x137 => Some(Button::UpperRightBumper),
            0x138 => Some(Button::LowerLeftTrigger),
            0x139 => Some(Button::LowerRightTrigger),
            0x13d => Some(Button::LeftStick),
            0x13e => Some(Button::RightStick),
            0x13b => Some(Button::Start),
            0x13a => Some(Button::Select),
            0x13c => Some(Button::Guide),
            0x220 => Some(Button::DPadUp),
            0x221 => Some(Button::DPadDown),
            0x222 => Some(Button::DPadLeft),
            0x223 => Some(Button::DPadRight),
            _ => None,
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
    UpperLeftBumper,
    UpperRightBumper,
    LowerLeftTrigger,
    LowerRightTrigger,
    DPadX,
    DPadY,
    Custom(u16),
}

impl Axis {
    /// Convert axis to Linux input event code
    pub fn to_ev_code(self) -> u16 {
        match self {
            Axis::LeftStickX => 0x00,        // ABS_X
            Axis::LeftStickY => 0x01,        // ABS_Y
            Axis::RightStickX => 0x03,       // ABS_RX
            Axis::RightStickY => 0x04,       // ABS_RY
            Axis::UpperLeftBumper => 0x13,   // ABS_HAT1Y
            Axis::UpperRightBumper => 0x12,  // ABS_HAT1X
            Axis::LowerLeftTrigger => 0x15,  // ABS_HAT2Y
            Axis::LowerRightTrigger => 0x14, // ABS_HAT2X
            Axis::DPadX => 0x10,             // ABS_HAT0X
            Axis::DPadY => 0x11,             // ABS_HAT0Y
            Axis::Custom(code) => code,
        }
    }

    /// Convert from Linux input event code to Axis
    pub fn from_ev_code(code: u16) -> Option<Self> {
        match code {
            0x00 => Some(Axis::LeftStickX),
            0x01 => Some(Axis::LeftStickY),
            0x03 => Some(Axis::RightStickX),
            0x04 => Some(Axis::RightStickY),
            0x13 => Some(Axis::UpperLeftBumper),
            0x12 => Some(Axis::UpperRightBumper),
            0x15 => Some(Axis::LowerLeftTrigger),
            0x14 => Some(Axis::LowerRightTrigger),
            0x10 => Some(Axis::DPadX),
            0x11 => Some(Axis::DPadY),
            _ => None,
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
    pub joystick_node: Option<String>,
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

/// Linux ABS input event structure (for absolute axes)
#[repr(C, packed)]
pub struct LinuxAbsEvent {
    pub value: i32,
    pub minimum: i32,
    pub maximum: i32,
    pub fuzz: i32,
    pub flat: i32,
    pub resolution: i32,
}

/// Linux joystick input event structure (for joystick nodes)
#[repr(C, packed)]
pub struct LinuxJsEvent {
    pub time: u32,
    pub value: i16,
    pub type_: u8,
    pub number: u8,
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
pub const EV_REL: u16 = 0x02;
pub const EV_ABS: u16 = 0x03;

pub const SYN_REPORT: u16 = 0;
