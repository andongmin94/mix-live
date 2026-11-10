//! Native VST3 editor sessions, separate from the microphone and its processing thread.
use crate::{InspectArgs, ParameterChange, Result, StateFile, read_child_report, parse_class_id, parse_parameter};
use clap::Args;
use serde_json::Value;
use std::{path::{Path, PathBuf}, process::{Command, Stdio}};

#[derive(Debug, Clone, Args)]
pub struct EditArgs {
    pub plugin: PathBuf,
    #[arg(long, value_parser = parse_class_id)]
    pub class_id: Option<String>,
    #[arg(long, default_value_t = 48_000, value_parser = clap::value_parser!(u32).range(8_000..=192_000))]
    pub sample_rate: u32,
    #[arg(long = "set", value_parser = parse_parameter)]
    pub parameters: Vec<ParameterChange>,
    #[arg(long)]
    pub state_in: Option<PathBuf>,
    /// Save to a NEW file after closing the editor normally. Omit to discard edits.
    #[arg(long)]
    pub state_out: Option<PathBuf>,
}

impl EditArgs {
    pub fn plugin_options(&self) -> InspectArgs {
        InspectArgs { plugin: self.plugin.clone(), class_id: self.class_id.clone(), sample_rate: self.sample_rate,
            parameters: self.parameters.clone(), state_in: self.state_in.clone(), state_out: self.state_out.clone(),
            // Reuse selection validation, not the inspection worker's deadline.
            timeout_seconds: 30 }
    }
}

/// The child owns both the native editor and plugin on its main thread. The parent waits
/// for explicit window closure, not a short probe timeout, then publishes a requested state.
pub fn edit(executable: &Path, options: &EditArgs) -> Result<Value> {
    let selection = options.plugin_options();
    selection.validate()?;
    let directory = tempfile::tempdir()?;
    let staged = directory.path().join("edited.mixlive-vst3.json");
    let report_path = directory.path().join("report.json");
    let mut command = Command::new(executable);
    command.arg("__edit");
    selection.append_plugin_args(&mut command);
    command.arg("--sample-rate").arg(options.sample_rate.to_string()).arg("--report").arg(&report_path);
    if options.state_out.is_some() { command.arg("--state-out").arg(&staged); }
    command.stdin(Stdio::null()).stdout(Stdio::null()).stderr(Stdio::inherit());
    let status = command.status()?;
    let mut report = read_child_report(status, &report_path)?;
    if report["operation"] != "edit" || report["editor_closed_normally"] != true {
        return Err("editor did not finish normally; no state was published".into());
    }
    if let Some(path) = &options.state_out {
        let state = StateFile::read(&staged)?;
        if report["plugin"]["uid"].as_str().is_none_or(|id| !id.eq_ignore_ascii_case(&state.class_id)) {
            return Err("editor state does not match its reported plugin class".into());
        }
        state.save_new(path)?;
        report["state_out"] = serde_json::to_value(path)?;
    }
    Ok(report)
}

#[cfg(windows)]
pub use native::{edit_in_child, flush_editor_changes};

#[cfg(windows)]
mod native {
    use super::*;
    use crate::host;
    use serde_json::json;
    use std::{sync::{Arc, Mutex}, time::Duration};
    use vst3_host::{Plugin, PluginWindow, plugin::ProcessMode};

    /// Main/control-thread only, with no concurrent audio processing. Forward editor edits
    /// and consume them in VST3's zero-sample parameter-flush call before saving state.
    /// No microphone, output device, or audible/synthetic audio block is used.
    pub fn flush_editor_changes(plugin: &mut Plugin) -> Result<usize> {
        plugin.service_host_requests()?;
        let edits = plugin.get_parameter_changes();
        let count = edits.len();
        for (id, value) in edits { plugin.set_parameter(id, value)?; }
        // Use the host's current bus allocation contract, but submit ZERO samples.
        // The host removes audio bus pointers/counts for this parameter-only call.
        let mut buffers = plugin.create_bus_audio_buffers(1)?;
        buffers.block_size = 0;
        plugin.process_bus_audio(&mut buffers)?;
        // This tool does not build an automation history. Drain the separate gesture log.
        let _ = plugin.take_parameter_edits();
        Ok(count)
    }

    /// Called only by the editor child on its executable's main thread.
    pub fn edit_in_child(options: &EditArgs) -> Result<Value> {
        let mut plugin = host::load_configured(&options.plugin_options())?;
        if !plugin.has_editor() { return Err("this VST3 has no native editor; use inspect --set instead".into()); }
        plugin.set_process_mode(ProcessMode::Realtime)?;
        plugin.start_processing()?;
        let shared = Arc::new(Mutex::new(plugin));
        let mut window = PluginWindow::new(shared.clone());
        let session = (|| -> Result<usize> {
            window.open()?;
            let mut changes = 0;
            loop {
                // Never hold the plugin mutex while dispatching Windows messages: a plugin
                // can synchronously call back into the host during repaint/resize/close.
                if !messages::pump() { return Err("editor session was interrupted; no state was saved".into()); }
                window.service_platform_events()?;
                {
                    let mut plugin = shared.lock().map_err(|_| "editor plugin lock was poisoned")?;
                    changes += flush_editor_changes(&mut plugin)?;
                }
                if window.closed_by_user() { break; }
                std::thread::sleep(Duration::from_millis(16));
            }
            Ok(changes)
        })();
        // IPlugView::removed must run while its parent HWND is still alive.
        drop(window);
        let (report, state) = {
            let mut plugin = shared.lock().map_err(|_| "editor plugin lock was poisoned")?;
            let flushed = if session.is_ok() { flush_editor_changes(&mut plugin).map(|_| ()) } else { Ok(()) };
            let stopped = plugin.stop_processing();
            let changes = session?;
            flushed?;
            stopped?;
            let state = if options.state_out.is_some() {
                Some(StateFile { schema: 1, class_id: plugin.info().uid.clone(), state: plugin.save_state()? })
            } else { None };
            let report = json!({ "schema": 1, "operation": "edit", "plugin": plugin.info(),
                "parameters": plugin.get_parameters()?, "sample_rate": options.sample_rate,
                "editor_closed_normally": true, "editor_parameter_changes": changes,
                "state_loaded": options.state_in.is_some(),
                "state_saved_bytes": state.as_ref().map(|state| state.state.len()),
                "live_audio_connected": false,
                "note": "Native editor and parameter-only processing; no audio devices or microphone were opened. Live VST3 processing is not connected." });
            (report, state)
        };
        // Native teardown precedes staging; the parent publishes only after this child exits.
        drop(shared);
        if let (Some(path), Some(state)) = (&options.state_out, state) { state.save_new(path)?; }
        Ok(report)
    }

    // The only unsafe code in this crate is the small Win32 message-dispatch boundary.
    // Window creation, editor attachment and destruction remain in the existing host library.
    #[allow(unsafe_code)]
    mod messages {
        use winapi::um::winuser::{DispatchMessageW, MSG, PM_REMOVE, PeekMessageW, TranslateMessage, WM_QUIT};

        pub(super) fn pump() -> bool {
            // SAFETY: MSG is an initialized POD output buffer, the null HWND selects this
            // calling thread's queue, and each retrieved MSG is dispatched before reuse.
            // The caller is the child main thread and keeps all plugin windows alive.
            unsafe {
                let mut message: MSG = std::mem::zeroed();
                while PeekMessageW(&mut message, std::ptr::null_mut(), 0, 0, PM_REMOVE) != 0 {
                    if message.message == WM_QUIT { return false; }
                    TranslateMessage(&message);
                    DispatchMessageW(&message);
                }
            }
            true
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::{Cli, Command as CliCommand};
    use clap::Parser;

    #[test]
    fn editor_requires_explicit_save_and_has_no_probe_deadline() {
        let cli = Cli::try_parse_from(["plugins", "edit", "effect.vst3"]).unwrap();
        let CliCommand::Edit(options) = cli.command else { panic!("expected edit") };
        assert!(options.state_out.is_none());
        assert!(Cli::try_parse_from(["plugins", "edit", "effect.vst3", "--timeout-seconds", "30"]).is_err());
        assert!(Cli::try_parse_from(["plugins", "edit", "effect.vst3", "--input", "mic"]).is_err());
        assert!(Cli::try_parse_from(["plugins", "edit", "effect.vst3", "--sample-rate", "0"]).is_err());
    }

    #[test]
    fn existing_state_is_rejected_before_editor_process_starts() {
        let directory = tempfile::tempdir().unwrap();
        let plugin = directory.path().join("effect.vst3");
        let saved = directory.path().join("saved.json");
        std::fs::write(&plugin, b"not loaded").unwrap();
        std::fs::write(&saved, b"keep this file").unwrap();
        let options = EditArgs { plugin, class_id: None, sample_rate: 48_000, parameters: vec![],
            state_in: None, state_out: Some(saved.clone()) };
        let error = edit(Path::new("must-not-be-launched.exe"), &options).unwrap_err();
        assert!(error.to_string().contains("already exists"));
        assert_eq!(std::fs::read(saved).unwrap(), b"keep this file");
    }
}
