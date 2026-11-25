//! Portable descriptions of the WASAPI endpoints available when the list was refreshed.
//! IDs identify devices; names are only display labels and may be duplicated.

#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct DeviceCatalog {
    pub inputs: Vec<DeviceInfo>,
    pub outputs: Vec<DeviceInfo>,
    pub apps: Vec<AppInfo>,
}

/// A process-tree identity observed in the current catalog, never persisted in a Scene.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct ProcessTarget {
    pub pid: u32,
    pub started: u64,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct AppInfo {
    pub path: String,
    /// Multiple independent roots or a tree containing MixLive are not selectable.
    pub target: Result<ProcessTarget, String>,
    /// Ancestor PIDs observed in this same snapshot; used only to reject overlapping trees.
    pub ancestors: Vec<u32>,
}

/// Match every selected path against one catalog snapshot. Overlapping process trees
/// would capture the same audio twice, so reject them before native activation.
pub fn resolve_apps(sources: &[crate::config::AppSource], catalog: &[AppInfo]) -> Result<Vec<ProcessTarget>, String> {
    let mut selected: Vec<(&AppInfo, ProcessTarget)> = Vec::with_capacity(sources.len());
    for source in sources {
        let app = catalog.iter().find(|app| app.path == source.path)
            .ok_or_else(|| format!("application is not running: {}", source.path))?;
        let target = *app.target.as_ref().map_err(Clone::clone)?;
        if selected.iter().any(|(other, previous)| previous.pid == target.pid
            || app.ancestors.contains(&previous.pid) || other.ancestors.contains(&target.pid)) {
            return Err(format!("overlapping application process trees: {}", source.path));
        }
        selected.push((app, target));
    }
    Ok(selected.into_iter().map(|(_, target)| target).collect())
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
