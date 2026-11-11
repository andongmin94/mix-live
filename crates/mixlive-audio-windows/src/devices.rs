//! Portable descriptions of the WASAPI endpoints available when the list was refreshed.
//! IDs identify devices; names are only display labels and may be duplicated.

#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct DeviceCatalog {
    pub inputs: Vec<DeviceInfo>,
    pub outputs: Vec<DeviceInfo>,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct DeviceInfo {
    pub id: String,
    pub name: String,
    /// The endpoint's default configuration, checked against the current engine's support.
    /// A failed or unsupported configuration stays visible with its reason.
    pub config: Result<DeviceConfig, String>,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct DeviceConfig {
    pub channels: u16,
    pub sample_rate: u32,
    pub sample_format: String,
}
