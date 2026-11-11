//! Ordered vocal VST3 chains. Selection and file I/O stay off audio callbacks.
use clap::Args;
use serde::{Deserialize, Serialize};
use std::{fs::File, io::{BufReader, Write}, path::{Path, PathBuf}};

#[derive(Args, Clone, Debug, Default)]
pub struct PluginArgs {
    /// Append a trusted stereo VST3 in processing order. Repeat for multiple effects.
    #[arg(long, global = true, conflicts_with = "vst3_chain")]
    pub vst3: Vec<PathBuf>,
    /// Restore an ordered MixLive chain, including each instance's saved state.
    #[arg(long, global = true, conflicts_with = "vst3")]
    pub vst3_chain: Option<PathBuf>,
    /// Open the live editor for this one-based slot. Repeat to open more than one.
    #[arg(long, global = true, value_parser = clap::value_parser!(u32).range(1..))]
    pub vst3_editor: Vec<u32>,
    /// Save the entire chain to a NEW file after a successful stop. Never overwrites.
    #[arg(long, global = true)]
    pub vst3_chain_out: Option<PathBuf>,
}

impl PluginArgs {
    pub fn enabled(&self) -> bool { !self.vst3.is_empty() || self.vst3_chain.is_some() }

    pub fn validate(&self) -> Result<(), String> {
        if !self.vst3.is_empty() && self.vst3_chain.is_some() {
            return Err("choose repeated --vst3 paths OR --vst3-chain, not both".into());
        }
        if !self.enabled() && (!self.vst3_editor.is_empty() || self.vst3_chain_out.is_some()) {
            return Err("VST3 editor/save options require an explicit chain".into());
        }
        for path in self.vst3.iter().chain(self.vst3_chain.iter()).chain(self.vst3_chain_out.iter()) {
            if path.as_os_str().is_empty() { return Err("VST3 paths must not be empty".into()); }
        }
        for (index, slot) in self.vst3_editor.iter().enumerate() {
            if *slot == 0 || self.vst3_editor[..index].contains(slot) {
                return Err("editor slots must be distinct one-based indices".into());
            }
        }
        if let Some(path) = &self.vst3_chain_out { require_new_file(path)?; }
        Ok(())
    }

    /// Resolve configuration before loading any native library or starting device streams.
    pub(crate) fn resolve(&self) -> Result<Vec<EffectSpec>, String> {
        self.validate()?;
        let effects = match &self.vst3_chain {
            Some(path) => ChainFile::read(path)?.effects,
            None => self.vst3.iter().map(|path| EffectSpec {
                plugin: path.clone(), class_id: None, state: None,
            }).collect(),
        };
        if self.vst3_editor.iter().any(|&slot| slot as usize > effects.len()) {
            return Err("requested editor slot is outside this VST3 chain".into());
        }
        Ok(effects)
    }
}

/// Repeated paths are intentional: each entry is a separate native instance and state.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub(crate) struct EffectSpec {
    pub plugin: PathBuf,
    pub class_id: Option<String>,
    pub state: Option<Vec<u8>>,
}

#[derive(Debug, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub(crate) struct ChainFile {
    pub schema: u32,
    pub effects: Vec<EffectSpec>,
}
impl ChainFile {
    fn validate(&self) -> Result<(), String> {
        if self.schema != 1 { return Err("unsupported MixLive VST3 chain schema".into()); }
        if self.effects.is_empty() { return Err("a VST3 chain needs at least one effect".into()); }
        for (index, effect) in self.effects.iter().enumerate() {
            if effect.plugin.as_os_str().is_empty() {
                return Err(format!("VST3 slot {} has an empty plugin path", index + 1));
            }
            if effect.class_id.as_ref().is_some_and(|id| id.len() != 32 || !id.bytes().all(|b| b.is_ascii_hexdigit())) {
                return Err(format!("VST3 slot {} needs a 32-digit hexadecimal class ID", index + 1));
            }
            if effect.state.is_some() && effect.class_id.is_none() {
                return Err(format!("VST3 slot {}: saved state requires its audio class ID", index + 1));
            }
        }
        Ok(())
    }

    pub fn read(path: &Path) -> Result<Self, String> {
        let path = path.canonicalize().map_err(|e| e.to_string())?;
        let mut chain: Self = serde_json::from_reader(BufReader::new(File::open(&path).map_err(|e| e.to_string())?))
            .map_err(|e| e.to_string())?;
        chain.validate()?;
        let parent = path.parent().ok_or("chain file has no parent directory")?;
        for effect in &mut chain.effects {
            if effect.plugin.is_relative() { effect.plugin = parent.join(&effect.plugin); }
        }
        Ok(chain)
    }

    /// Publish all slot states as ONE document only after every native instance has shut down.
    pub fn save_new(&self, path: &Path) -> Result<(), String> {
        self.validate()?;
        require_new_file(path)?;
        let parent = path.parent().filter(|p| !p.as_os_str().is_empty()).unwrap_or_else(|| Path::new("."));
        let mut temporary = tempfile::NamedTempFile::new_in(parent).map_err(|e| e.to_string())?;
        serde_json::to_writer_pretty(&mut temporary, self).map_err(|e| e.to_string())?;
        temporary.write_all(b"\n").map_err(|e| e.to_string())?;
        temporary.as_file().sync_all().map_err(|e| e.to_string())?;
        temporary.persist_noclobber(path).map_err(|e| e.to_string())?;
        Ok(())
    }
}

fn require_new_file(path: &Path) -> Result<(), String> {
    match path.symlink_metadata() {
        Ok(_) => Err("VST3 chain output already exists; choose a new file".into()),
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => Ok(()),
        Err(error) => Err(error.to_string()),
    }
}

#[cfg(windows)]
#[path = "plugin_native.rs"]
pub(crate) mod native;

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    #[test]
    fn chain_selection_is_ordered_and_editor_indices_are_explicit() {
        assert!(PluginArgs::default().resolve().unwrap().is_empty());
        let mut args = PluginArgs { vst3: vec!["eq.vst3".into(), "room.vst3".into(), "eq.vst3".into()],
            vst3_editor: vec![3, 1], ..PluginArgs::default() };
        let effects = args.resolve().unwrap();
        assert_eq!(effects.iter().map(|s| &s.plugin).collect::<Vec<_>>(), args.vst3.iter().collect::<Vec<_>>());
        for slots in [vec![0], vec![4], vec![1, 1]] { args.vst3_editor = slots; assert!(args.resolve().is_err()); }
        args.vst3_editor.clear();
        args.vst3_chain = Some("voice.json".into());
        assert!(args.validate().is_err());
        for args in [PluginArgs { vst3_editor: vec![1], ..PluginArgs::default() },
            PluginArgs { vst3_chain_out: Some("out.json".into()), ..PluginArgs::default() },
            PluginArgs { vst3: vec![PathBuf::new()], ..PluginArgs::default() }] {
            assert!(args.validate().is_err());
        }
    }

    #[test]
    fn chain_files_keep_independent_states_relative_paths_and_never_overwrite() {
        let directory = tempfile::tempdir().unwrap();
        let path = directory.path().join("보컬 체인.json");
        let first = EffectSpec { plugin: "effect.vst3".into(), class_id: Some("0123456789ABCDEF0123456789abcdef".into()), state: Some(vec![1, 2]) };
        let second = EffectSpec { state: Some(vec![3, 4]), ..first.clone() };
        let chain = ChainFile { schema: 1, effects: vec![first, second] };
        chain.save_new(&path).unwrap();
        let original = std::fs::read(&path).unwrap();
        assert!(chain.save_new(&path).unwrap_err().contains("already exists"));
        assert_eq!(std::fs::read(&path).unwrap(), original);
        let loaded = ChainFile::read(&path).unwrap();
        assert_eq!(loaded.effects[0].plugin, path.canonicalize().unwrap().parent().unwrap().join("effect.vst3"));
        assert_eq!(loaded.effects[0].state.as_deref(), Some([1, 2].as_slice()));
        assert_eq!(loaded.effects[1].state.as_deref(), Some([3, 4].as_slice()));
        let args = PluginArgs { vst3_chain: Some(path.clone()), vst3_editor: vec![2], ..PluginArgs::default() };
        assert_eq!(args.resolve().unwrap(), loaded.effects);
        let args = PluginArgs { vst3_chain_out: Some(path), ..args };
        assert!(args.validate().is_err());
        assert_eq!(std::fs::read_dir(directory.path()).unwrap().count(), 1);
    }

    #[test]
    fn malformed_chain_files_are_rejected_without_loading_plugins() {
        let directory = tempfile::tempdir().unwrap();
        let path = directory.path().join("chain.json");
        for document in [json!({"schema": 99, "effects": [{"plugin": "effect.vst3"}]}),
            json!({"schema": 1, "effects": []}),
            json!({"schema": 1, "effects": [{"plugin": ""}]}),
            json!({"schema": 1, "effects": [{"plugin": "x", "class_id": "wrong"}]}),
            json!({"schema": 1, "effects": [{"plugin": "x", "state": [1]}]}),
            json!({"schema": 1, "effects": [{"plugin": "x", "typo": true}]}),
            json!({"schema": 1, "effects": [{"plugin": "x"}], "typo": true})] {
            std::fs::write(&path, serde_json::to_vec(&document).unwrap()).unwrap();
            assert!(ChainFile::read(&path).is_err(), "accepted {document}");
        }
    }
}
