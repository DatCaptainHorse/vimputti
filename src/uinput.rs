// Uinput constants and structures
pub const UI_SET_EVBIT: u64 = 0x40045564;
pub const UI_SET_KEYBIT: u64 = 0x40045565;
pub const UI_SET_ABSBIT: u64 = 0x40045567;
pub const UI_SET_FFBIT: u64 = 0x4004556b;
pub const UI_DEV_SETUP: u64 = 0x405c5503;
pub const UI_DEV_CREATE: u64 = 0x5501;
pub const UI_DEV_DESTROY: u64 = 0x5502;
pub const UI_ABS_SETUP: u64 = 0x401c5504;

// Get sysfs name for uinput device
pub fn ui_get_sysname(len: usize) -> u64 {
    // _IOC(_IOC_READ, 'U', 0x2c, len)
    0x80000000 | ((len as u64 & 0x1fff) << 16) | (b'U' as u64) << 8 | 0x2c
}

#[repr(C)]
#[derive(Debug, Clone, Copy)]
pub struct input_id {
    pub bustype: u16,
    pub vendor: u16,
    pub product: u16,
    pub version: u16,
}

#[repr(C)]
#[derive(Debug)]
pub struct uinput_setup {
    pub id: input_id,
    pub name: [u8; 80],
    pub ff_effects_max: u32,
}

#[repr(C)]
#[derive(Debug, Clone, Copy)]
pub struct input_absinfo {
    pub value: i32,
    pub minimum: i32,
    pub maximum: i32,
    pub fuzz: i32,
    pub flat: i32,
    pub resolution: i32,
}

#[repr(C)]
#[derive(Debug)]
pub struct uinput_abs_setup {
    pub code: u16,
    pub absinfo: input_absinfo,
}
