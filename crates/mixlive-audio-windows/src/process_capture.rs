//! Explicit application-loopback capture, independent of playback endpoint identity.
//! The COM client and fixed packet buffer live on one capture thread. No recording.
use crate::{
    bridge::{Fault, Health, OutputWriter},
    config::InputMap,
    devices::{AppInfo, ProcessTarget},
};
use std::{
    collections::BTreeMap,
    sync::{Arc, atomic::{AtomicBool, Ordering}, mpsc::{self, SyncSender}},
    thread::{self, JoinHandle},
};
use sysinfo::{ProcessRefreshKind, RefreshKind, System, UpdateKind};
use wasapi::{AudioClient, Direction, SampleType, StreamMode, WasapiError, WaveFormat};

pub(crate) const RATE: u32 = 48_000;
const FRAME_BYTES: usize = 8; // explicitly negotiated float32 stereo, little-endian on Windows

struct ProcessRow {
    target: ProcessTarget,
    parent: Option<u32>,
    path: Option<String>,
}

fn ancestors(rows: &BTreeMap<u32, &ProcessRow>, mut pid: u32) -> Vec<u32> {
    let mut result = Vec::new();
    while let Some(row) = rows.get(&pid) {
        let Some(parent) = row.parent.and_then(|id| rows.get(&id)) else { break; };
        // A reused parent PID is not an ancestor. Also reject cycles in a snapshot.
        if parent.target.started > row.target.started || parent.target.pid == pid
            || result.contains(&parent.target.pid) { break; }
        pid = parent.target.pid;
        result.push(pid);
    }
    result
}

fn catalog_from_rows(rows: &[ProcessRow], own_pid: u32) -> Vec<AppInfo> {
    let indexed: BTreeMap<_, _> = rows.iter().map(|row| (row.target.pid, row)).collect();
    let mut own_tree = ancestors(&indexed, own_pid);
    own_tree.push(own_pid);
    let mut roots: BTreeMap<String, Vec<ProcessTarget>> = BTreeMap::new();
    for row in rows {
        let Some(path) = &row.path else { continue; };
        if row.target.pid == 0 || ancestors(&indexed, row.target.pid).iter()
            .any(|pid| indexed[pid].path.as_ref() == Some(path)) { continue; }
        roots.entry(path.clone()).or_default().push(row.target);
    }
    roots.into_iter().map(|(path, candidates)| {
        let target = if candidates.iter().any(|candidate| own_tree.contains(&candidate.pid)) {
            Err("this process tree contains MixLive; capture would feed its own output back".into())
        } else if let [candidate] = candidates.as_slice() {
            Ok(*candidate)
        } else {
            Err(format!("{} independent process trees use this path; leave only the intended app instance running", candidates.len()))
        };
        AppInfo { path, target }
    }).collect()
}

/// Request only executable path, PID, parent and start time.
pub(crate) fn app_catalog() -> Vec<AppInfo> {
    let system = System::new_with_specifics(RefreshKind::new()
        .with_processes(ProcessRefreshKind::new().with_exe(UpdateKind::Always)));
    let rows: Vec<_> = system.processes().values().map(|process| ProcessRow {
        target: ProcessTarget { pid: process.pid().as_u32(), started: process.start_time() },
        parent: process.parent().map(|id| id.as_u32()),
        path: process.exe().filter(|path| path.is_absolute())
            .and_then(|path| path.to_str()).map(str::to_owned),
    }).collect();
    catalog_from_rows(&rows, std::process::id())
}

pub(crate) fn resolve(path: &str) -> Result<ProcessTarget, String> {
    app_catalog().into_iter().find(|app| app.path == path)
        .ok_or_else(|| format!("application is not running or its exact executable path is unavailable: {path}"))?
        .target
}

/// A running client whose stop/join follows the existing device-stream teardown.
pub(crate) struct ProcessCapture {
    stop: Arc<AtomicBool>,
    thread: Option<JoinHandle<()>>,
    health: Arc<Health>,
}

impl ProcessCapture {
    pub fn start(path: String, target: ProcessTarget, map: InputMap,
        writer: OutputWriter, health: Arc<Health>) -> Result<Self, String> {
        let stop = Arc::new(AtomicBool::new(false));
        let (ready_tx, ready_rx) = mpsc::sync_channel(1);
        let thread_stop = stop.clone();
        let thread_health = health.clone();
        let thread = thread::Builder::new().name("mixlive-app-capture".into()).spawn(move || {
            let result = capture(&path, target, map, writer, &thread_stop, &thread_health, &ready_tx);
            if let Err(error) = result {
                let _ = ready_tx.try_send(Err(error));
                thread_health.fail(Fault::Application);
            }
        }).map_err(|error| error.to_string())?;
        let owner = Self { stop, thread: Some(thread), health };
        match ready_rx.recv() {
            Ok(Ok(())) => Ok(owner),
            Ok(Err(error)) => Err(error),
            Err(_) => Err("application capture thread exited during startup".into()),
        }
    }
}

impl Drop for ProcessCapture {
    fn drop(&mut self) {
        self.stop.store(true, Ordering::Relaxed);
        if let Some(thread) = self.thread.take() {
            if thread.join().is_err() { self.health.fail(Fault::Application); }
        }
    }
}

fn capture(path: &str, target: ProcessTarget, map: InputMap, mut writer: OutputWriter,
    stop: &AtomicBool, health: &Health, ready: &SyncSender<Result<(), String>>) -> Result<(), String> {
    // Recheck the resolved identity immediately before opening the process-loopback client.
    if resolve(path)? != target { return Err("application process changed before capture started; start again".into()); }
    wasapi::initialize_mta().ok().map_err(|error| error.to_string())?;
    let result = (|| -> Result<(), String> {
        let mut client = AudioClient::new_application_loopback_client(target.pid, true)
            .map_err(|error| error.to_string())?;
        let format = WaveFormat::new(32, 32, SampleType::Float, RATE as usize, 2, None);
        client.initialize_client(&format, Direction::Capture,
            StreamMode::EventsShared { autoconvert: true, buffer_duration_hns: 0 }, None)
            .map_err(|error| error.to_string())?;
        let event = client.set_get_eventhandle().map_err(|error| error.to_string())?;
        let capture = client.get_audiocaptureclient().map_err(|error| error.to_string())?;
        // Process loopback's reported buffer size is unreliable. Allocate bounded scratch once.
        let mut packet = vec![0_u8; RATE as usize * FRAME_BYTES];
        client.start_stream().map_err(|error| error.to_string())?;
        let running = (|| -> Result<(), String> {
            ready.send(Ok(())).map_err(|_| "application capture startup was cancelled")?;
            let mut received_packet = false;
            while !stop.load(Ordering::Relaxed) && !health.failed() {
                let pending = capture.get_next_packet_size().map_err(|error| error.to_string())?
                    .ok_or("application capture is not a shared stream")?;
                if pending == 0 {
                    match event.wait_for_event(50) {
                        Ok(()) | Err(WasapiError::EventTimeout) => continue,
                        Err(error) => return Err(error.to_string()),
                    }
                }
                let (frames, info) = capture.read_from_device(&mut packet).map_err(|error| error.to_string())?;
                if frames == 0 { continue; }
                if received_packet && info.flags.data_discontinuity {
                    return Err("application capture data discontinuity".into());
                }
                received_packet = true;
                // The library zeroes silent packets. Only read the returned frames.
                for frame in packet[..frames as usize * FRAME_BYTES].chunks_exact(FRAME_BYTES) {
                    let [left, right] = decode_frame(frame, map);
                    writer.push([left, right, 0.0, 0.0]);
                    if health.failed() { break; }
                }
            }
            Ok(())
        })();
        let stopped = client.stop_stream().map_err(|error| error.to_string());
        running?;
        stopped
    })();
    // All COM-owned interfaces are dropped before uninitializing this apartment.
    wasapi::deinitialize();
    result
}

fn decode_frame(bytes: &[u8], map: InputMap) -> [f32; 2] {
    let stereo = [
        f32::from_le_bytes(bytes[..4].try_into().expect("float32 channel")),
        f32::from_le_bytes(bytes[4..8].try_into().expect("float32 channel")),
    ];
    map.read(&stereo)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn catalog_groups_children_rejects_ambiguity_and_own_tree_and_ignores_reused_parent() {
        let row = |pid, parent, started, path: &str| ProcessRow {
            target: ProcessTarget { pid, started }, parent, path: Some(path.into()),
        };
        let mut rows = vec![
            row(1, None, 1, "launcher.exe"),
            row(2, Some(1), 2, "chrome.exe"),
            row(3, Some(2), 3, "chrome.exe"),
            row(4, Some(3), 4, "chrome.exe"),
            row(5, Some(1), 5, "mixlive.exe"),
            row(6, Some(5), 1, "other.exe"),
        ];
        let apps = catalog_from_rows(&rows, 5);
        let find = |apps: &[AppInfo], path: &str| apps.iter().find(|app| app.path == path).unwrap().target.clone();
        assert_eq!(find(&apps, "chrome.exe").unwrap().pid, 2);
        assert!(find(&apps, "launcher.exe").is_err());
        assert!(find(&apps, "mixlive.exe").is_err());
        assert_eq!(find(&apps, "other.exe").unwrap().pid, 6);
        rows.push(row(7, Some(1), 7, "chrome.exe"));
        assert!(find(&catalog_from_rows(&rows, 5), "chrome.exe").is_err());
    }

    #[test]
    fn float_stereo_packets_keep_selected_order_and_mono_duplication() {
        let frame: Vec<u8> = [0.25_f32, -0.5].into_iter().flat_map(f32::to_le_bytes).collect();
        assert_eq!(decode_frame(&frame, InputMap::new(&[1, 2], 2).unwrap()), [0.25, -0.5]);
        assert_eq!(decode_frame(&frame, InputMap::new(&[2, 1], 2).unwrap()), [-0.5, 0.25]);
        assert_eq!(decode_frame(&frame, InputMap::new(&[2], 2).unwrap()), [-0.5; 2]);
    }
}
