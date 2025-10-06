use crate::protocol::*;

/// Pre-configured controller templates
pub struct ControllerTemplates;

impl ControllerTemplates {
    /// Xbox 360 Controller
    pub fn xbox360() -> DeviceConfig {
        DeviceConfig {
            name: "Microsoft X-Box 360 pad".to_string(),
            vendor_id: 0x045e,
            product_id: 0x028e,
            version: 0x0110,
            bustype: BusType::Usb,
            buttons: vec![
                Button::A,
                Button::B,
                Button::X,
                Button::Y,
                Button::LeftBumper,
                Button::RightBumper,
                Button::Select,     // Back
                Button::Start,      // Start
                Button::Guide,      // Xbox button
                Button::LeftStick,  // Left stick click
                Button::RightStick, // Right stick click
            ],
            axes: vec![
                AxisConfig::new(Axis::LeftStickX, -32768, 32767),
                AxisConfig::new(Axis::LeftStickY, -32768, 32767),
                AxisConfig::new(Axis::RightStickX, -32768, 32767),
                AxisConfig::new(Axis::RightStickY, -32768, 32767),
                AxisConfig::new(Axis::LeftTrigger, -32768, 32767),
                AxisConfig::new(Axis::RightTrigger, -32768, 32767),
                AxisConfig::new(Axis::DPadX, -1, 1),
                AxisConfig::new(Axis::DPadY, -1, 1),
            ],
        }
    }

    /// Xbox One Controller
    pub fn xbox_one() -> DeviceConfig {
        DeviceConfig {
            name: "Microsoft X-Box One pad".to_string(),
            vendor_id: 0x045e,
            product_id: 0x02ea,
            version: 0x0408,
            bustype: BusType::Usb,
            buttons: vec![
                Button::A,
                Button::B,
                Button::X,
                Button::Y,
                Button::LeftBumper,
                Button::RightBumper,
                Button::Select,
                Button::Start,
                Button::Guide,
                Button::LeftStick,
                Button::RightStick,
            ],
            axes: vec![
                AxisConfig::new(Axis::LeftStickX, -32768, 32767),
                AxisConfig::new(Axis::LeftStickY, -32768, 32767),
                AxisConfig::new(Axis::RightStickX, -32768, 32767),
                AxisConfig::new(Axis::RightStickY, -32768, 32767),
                AxisConfig::new(Axis::LeftTrigger, -32768, 32767),
                AxisConfig::new(Axis::RightTrigger, -32768, 32767),
                AxisConfig::new(Axis::DPadX, -1, 1),
                AxisConfig::new(Axis::DPadY, -1, 1),
            ],
        }
    }

    /// PlayStation 4 Controller (DualShock 4)
    pub fn ps4() -> DeviceConfig {
        DeviceConfig {
            name: "Sony Interactive Entertainment Wireless Controller".to_string(),
            vendor_id: 0x054c,
            product_id: 0x09cc,
            version: 0x8111,
            bustype: BusType::Usb,
            buttons: vec![
                Button::X,            // Cross (mapped to X)
                Button::A,            // Circle (mapped to A)
                Button::B,            // Square (mapped to B)
                Button::Y,            // Triangle (mapped to Y)
                Button::LeftBumper,   // L1
                Button::RightBumper,  // R1
                Button::LeftTrigger,  // L2
                Button::RightTrigger, // R2
                Button::Select,       // Share
                Button::Start,        // Options
                Button::Guide,        // PS button
                Button::LeftStick,    // L3
                Button::RightStick,   // R3
            ],
            axes: vec![
                AxisConfig::new(Axis::LeftStickX, -32768, 32767),
                AxisConfig::new(Axis::LeftStickY, -32768, 32767),
                AxisConfig::new(Axis::RightStickX, -32768, 32767),
                AxisConfig::new(Axis::RightStickY, -32768, 32767),
                AxisConfig::new(Axis::LeftTrigger, -32768, 32767),
                AxisConfig::new(Axis::RightTrigger, -32768, 32767),
                AxisConfig::new(Axis::DPadX, -1, 1),
                AxisConfig::new(Axis::DPadY, -1, 1),
            ],
        }
    }

    /// PlayStation 5 Controller (DualSense)
    pub fn ps5() -> DeviceConfig {
        DeviceConfig {
            name: "Sony Interactive Entertainment DualSense Wireless Controller".to_string(),
            vendor_id: 0x054c,
            product_id: 0x0ce6,
            version: 0x8111,
            bustype: BusType::Usb,
            buttons: vec![
                Button::X,            // Cross
                Button::A,            // Circle
                Button::B,            // Square
                Button::Y,            // Triangle
                Button::LeftBumper,   // L1
                Button::RightBumper,  // R1
                Button::LeftTrigger,  // L2
                Button::RightTrigger, // R2
                Button::Select,       // Create
                Button::Start,        // Options
                Button::Guide,        // PS button
                Button::LeftStick,    // L3
                Button::RightStick,   // R3
            ],
            axes: vec![
                AxisConfig::new(Axis::LeftStickX, -32768, 32767),
                AxisConfig::new(Axis::LeftStickY, -32768, 32767),
                AxisConfig::new(Axis::RightStickX, -32768, 32767),
                AxisConfig::new(Axis::RightStickY, -32768, 32767),
                AxisConfig::new(Axis::LeftTrigger, -32768, 32767),
                AxisConfig::new(Axis::RightTrigger, -32768, 32767),
                AxisConfig::new(Axis::DPadX, -1, 1),
                AxisConfig::new(Axis::DPadY, -1, 1),
            ],
        }
    }

    /// Nintendo Switch Pro Controller
    pub fn switch_pro() -> DeviceConfig {
        DeviceConfig {
            name: "Nintendo Switch Pro Controller".to_string(),
            vendor_id: 0x057e,
            product_id: 0x2009,
            version: 0x8111,
            bustype: BusType::Usb,
            buttons: vec![
                Button::B,            // A (Nintendo)
                Button::A,            // B (Nintendo)
                Button::Y,            // X (Nintendo)
                Button::X,            // Y (Nintendo)
                Button::LeftBumper,   // L
                Button::RightBumper,  // R
                Button::LeftTrigger,  // ZL
                Button::RightTrigger, // ZR
                Button::Select,       // Minus
                Button::Start,        // Plus
                Button::Guide,        // Home
                Button::LeftStick,    // Left stick click
                Button::RightStick,   // Right stick click
                Button::Custom(317),  // Capture button
            ],
            axes: vec![
                AxisConfig::new(Axis::LeftStickX, -32768, 32767),
                AxisConfig::new(Axis::LeftStickY, -32768, 32767),
                AxisConfig::new(Axis::RightStickX, -32768, 32767),
                AxisConfig::new(Axis::RightStickY, -32768, 32767),
                AxisConfig::new(Axis::DPadX, -1, 1),
                AxisConfig::new(Axis::DPadY, -1, 1),
            ],
        }
    }

    /// Generic USB gamepad (basic configuration)
    pub fn generic_gamepad() -> DeviceConfig {
        DeviceConfig {
            name: "Generic USB Gamepad".to_string(),
            vendor_id: 0x0079,
            product_id: 0x0006,
            version: 0x0110,
            bustype: BusType::Usb,
            buttons: vec![
                Button::A,
                Button::B,
                Button::X,
                Button::Y,
                Button::LeftBumper,
                Button::RightBumper,
                Button::Select,
                Button::Start,
            ],
            axes: vec![
                AxisConfig::new(Axis::LeftStickX, -32768, 32767),
                AxisConfig::new(Axis::LeftStickY, -32768, 32767),
                AxisConfig::new(Axis::RightStickX, -32768, 32767),
                AxisConfig::new(Axis::RightStickY, -32768, 32767),
            ],
        }
    }

    /// Steam Controller
    pub fn steam_controller() -> DeviceConfig {
        DeviceConfig {
            name: "Steam Controller".to_string(),
            vendor_id: 0x28de,
            product_id: 0x1142,
            version: 0x0111,
            bustype: BusType::Usb,
            buttons: vec![
                Button::A,
                Button::B,
                Button::X,
                Button::Y,
                Button::LeftBumper,
                Button::RightBumper,
                Button::LeftTrigger,
                Button::RightTrigger,
                Button::Select,
                Button::Start,
                Button::Guide,
                Button::LeftStick,
                Button::RightStick,
                Button::Custom(289), // Left pad click
                Button::Custom(290), // Right pad click
            ],
            axes: vec![
                AxisConfig::new(Axis::LeftStickX, -32768, 32767),
                AxisConfig::new(Axis::LeftStickY, -32768, 32767),
                AxisConfig::new(Axis::Custom(3), -32768, 32767), // Left pad X
                AxisConfig::new(Axis::Custom(4), -32768, 32767), // Left pad Y
                AxisConfig::new(Axis::Custom(5), -32768, 32767), // Right pad X
                AxisConfig::new(Axis::Custom(6), -32768, 32767), // Right pad Y
                AxisConfig::new(Axis::LeftTrigger, -32768, 32767),
                AxisConfig::new(Axis::RightTrigger, -32768, 32767),
            ],
        }
    }
}

/// Builder for creating custom controller configurations
pub struct ControllerBuilder {
    config: DeviceConfig,
}

impl ControllerBuilder {
    /// Start building a custom controller
    pub fn new(name: impl Into<String>) -> Self {
        Self {
            config: DeviceConfig {
                name: name.into(),
                vendor_id: 0x0000,
                product_id: 0x0000,
                version: 0x0100,
                bustype: BusType::Virtual,
                buttons: Vec::new(),
                axes: Vec::new(),
            },
        }
    }

    /// Set vendor ID
    pub fn vendor_id(mut self, vendor_id: u16) -> Self {
        self.config.vendor_id = vendor_id;
        self
    }

    /// Set product ID
    pub fn product_id(mut self, product_id: u16) -> Self {
        self.config.product_id = product_id;
        self
    }

    /// Set version
    pub fn version(mut self, version: u16) -> Self {
        self.config.version = version;
        self
    }

    /// Set bus type
    pub fn bustype(mut self, bustype: BusType) -> Self {
        self.config.bustype = bustype;
        self
    }

    /// Add a button
    pub fn button(mut self, button: Button) -> Self {
        self.config.buttons.push(button);
        self
    }

    /// Add multiple buttons
    pub fn buttons(mut self, buttons: impl IntoIterator<Item = Button>) -> Self {
        self.config.buttons.extend(buttons);
        self
    }

    /// Add an axis
    pub fn axis(mut self, axis: Axis, min: i32, max: i32) -> Self {
        self.config.axes.push(AxisConfig::new(axis, min, max));
        self
    }

    /// Add an axis with full configuration
    pub fn axis_config(mut self, config: AxisConfig) -> Self {
        self.config.axes.push(config);
        self
    }

    /// Add multiple axes
    pub fn axes(mut self, axes: impl IntoIterator<Item = AxisConfig>) -> Self {
        self.config.axes.extend(axes);
        self
    }

    /// Build the configuration
    pub fn build(self) -> DeviceConfig {
        self.config
    }
}

/// Convenience methods for common button sets
impl ControllerBuilder {
    /// Add standard face buttons (A, B, X, Y)
    pub fn face_buttons(self) -> Self {
        self.buttons([Button::A, Button::B, Button::X, Button::Y])
    }

    /// Add shoulder buttons (L1, R1, L2, R2)
    pub fn shoulder_buttons(self) -> Self {
        self.buttons([
            Button::LeftBumper,
            Button::RightBumper,
            Button::LeftTrigger,
            Button::RightTrigger,
        ])
    }

    /// Add standard menu buttons (Start, Select, Guide)
    pub fn menu_buttons(self) -> Self {
        self.buttons([Button::Start, Button::Select, Button::Guide])
    }

    /// Add stick click buttons
    pub fn stick_buttons(self) -> Self {
        self.buttons([Button::LeftStick, Button::RightStick])
    }

    /// Add D-pad buttons
    pub fn dpad_buttons(self) -> Self {
        self.buttons([
            Button::DPadUp,
            Button::DPadDown,
            Button::DPadLeft,
            Button::DPadRight,
        ])
    }

    /// Add standard dual analog sticks
    pub fn dual_analog_sticks(self) -> Self {
        self.axes([
            AxisConfig::new(Axis::LeftStickX, -32768, 32767),
            AxisConfig::new(Axis::LeftStickY, -32768, 32767),
            AxisConfig::new(Axis::RightStickX, -32768, 32767),
            AxisConfig::new(Axis::RightStickY, -32768, 32767),
        ])
    }

    /// Add analog triggers
    pub fn analog_triggers(self) -> Self {
        self.axes([
            AxisConfig::new(Axis::LeftTrigger, 0, 255),
            AxisConfig::new(Axis::RightTrigger, 0, 255),
        ])
    }

    /// Add D-pad as axes
    pub fn dpad_axes(self) -> Self {
        self.axes([
            AxisConfig::new(Axis::DPadX, -1, 1),
            AxisConfig::new(Axis::DPadY, -1, 1),
        ])
    }
}
