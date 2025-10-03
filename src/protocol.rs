use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, Serialize, Deserialize)]
pub enum DeviceCommand {
    New {
        ptr: u64,
    },
    SetName {
        ptr: u64,
        name: String,
    },
    SetPhys {
        ptr: u64,
        phys: String,
    },
    SetUniq {
        ptr: u64,
        uniq: String,
    },
    SetIdBustype {
        ptr: u64,
        bustype: u16,
    },
    SetIdVendor {
        ptr: u64,
        vendor: u16,
    },
    SetIdProduct {
        ptr: u64,
        product: u16,
    },
    SetIdVersion {
        ptr: u64,
        version: u16,
    },
    SetDriverVersion {
        ptr: u64,
        version: u32,
    },
    EnableEventType {
        ptr: u64,
        type_: u32,
    },
    EnableEventCode {
        ptr: u64,
        type_: u32,
        code: u32,
    },
    UinputCreateFromDevice {
        ptr: u64,
        uinput_ptr: u64,
    },
    Free {
        ptr: u64,
    },
    UinputDestroy {
        uinput_ptr: u64,
    },
    UinputWriteEvent {
        uinput_ptr: u64,
        type_: u32,
        code: u32,
        value: i32,
    },
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub enum DeviceResponse {
    Success,
    Error { message: String },
    UinputCreated { uinput_ptr: u64 },
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Message {
    pub id: String,
    pub command: DeviceCommand,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Response {
    pub id: String,
    pub response: DeviceResponse,
}
