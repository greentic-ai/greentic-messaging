use std::collections::HashSet;
use std::env;
use std::fs;
use std::path::{Path, PathBuf};

/// Configuration controlling which default messaging adapter packs to load.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct DefaultAdapterPacksConfig {
    /// Load all default packs shipped in `packs/messaging`.
    pub install_all: bool,
    /// Specific default pack ids to load (e.g., `["teams", "slack"]`).
    pub selected: Vec<String>,
}

impl DefaultAdapterPacksConfig {
    pub fn from_env() -> Self {
        let install_all = env::var("MESSAGING_INSTALL_ALL_DEFAULT_ADAPTER_PACKS")
            .map(|v| v.eq_ignore_ascii_case("true"))
            .unwrap_or(false);
        let selected = env::var("MESSAGING_DEFAULT_ADAPTER_PACKS")
            .ok()
            .map(|raw| {
                raw.split(',')
                    .filter_map(|s| {
                        let trimmed = s.trim();
                        if trimmed.is_empty() {
                            None
                        } else {
                            Some(trimmed.to_string())
                        }
                    })
                    .collect()
            })
            .unwrap_or_default();
        Self {
            install_all,
            selected,
        }
    }
}

/// Pack metadata mapped to its file path.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct DefaultAdapterPack {
    pub id: &'static str,
    pub filename: &'static str,
}

const DEFAULT_PACKS: &[DefaultAdapterPack] = &[
    DefaultAdapterPack {
        id: "slack",
        filename: "slack.yaml",
    },
    DefaultAdapterPack {
        id: "teams",
        filename: "teams.yaml",
    },
    DefaultAdapterPack {
        id: "webex",
        filename: "webex.yaml",
    },
    DefaultAdapterPack {
        id: "webchat",
        filename: "webchat.yaml",
    },
    DefaultAdapterPack {
        id: "whatsapp",
        filename: "whatsapp.yaml",
    },
    DefaultAdapterPack {
        id: "telegram",
        filename: "telegram.yaml",
    },
    DefaultAdapterPack {
        id: "local",
        filename: "local.yaml",
    },
];

/// Resolved pack path and raw contents.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct LoadedPack {
    pub id: String,
    pub path: PathBuf,
    pub raw: String,
}

/// Resolve pack file system paths without reading contents.
pub fn default_adapter_pack_paths(
    packs_root: &Path,
    config: &DefaultAdapterPacksConfig,
) -> Vec<PathBuf> {
    resolve_default_adapter_packs(config)
        .into_iter()
        .map(|pack| packs_root.join("messaging").join(pack.filename))
        .collect()
}

/// Additional adapter pack paths from env (`MESSAGING_ADAPTER_PACK_PATHS`, comma-separated).
pub fn adapter_pack_paths_from_env() -> Vec<PathBuf> {
    env::var("MESSAGING_ADAPTER_PACK_PATHS")
        .ok()
        .map(|raw| {
            raw.split(',')
                .filter_map(|s| {
                    let trimmed = s.trim();
                    if trimmed.is_empty() {
                        None
                    } else {
                        Some(PathBuf::from(trimmed))
                    }
                })
                .collect()
        })
        .unwrap_or_default()
}

/// Load and parse default adapter packs into an adapter registry.
pub fn load_default_adapter_registry(
    packs_root: &Path,
    config: &DefaultAdapterPacksConfig,
) -> anyhow::Result<crate::AdapterRegistry> {
    let paths = default_adapter_pack_paths(packs_root, config);
    crate::adapter_registry::load_adapters_from_pack_files(&paths)
        .map_err(|err| err.context("failed to load default messaging adapter packs"))
}

/// Resolve which default adapter packs should be loaded based on config.
pub fn resolve_default_adapter_packs(
    config: &DefaultAdapterPacksConfig,
) -> Vec<&'static DefaultAdapterPack> {
    if config.install_all {
        return DEFAULT_PACKS.iter().collect();
    }
    if config.selected.is_empty() {
        return Vec::new();
    }
    let selected: HashSet<String> = config
        .selected
        .iter()
        .map(|s| s.to_ascii_lowercase())
        .collect();
    DEFAULT_PACKS
        .iter()
        .filter(|pack| selected.contains(pack.id))
        .collect()
}

/// Load the default adapter pack files from a root directory.
///
/// `packs_root` should point at the directory containing `messaging/`.
pub fn load_default_adapter_packs_from(
    packs_root: &Path,
    config: &DefaultAdapterPacksConfig,
) -> std::io::Result<Vec<LoadedPack>> {
    let resolved = resolve_default_adapter_packs(config);
    let mut out = Vec::with_capacity(resolved.len());
    for pack in resolved {
        let path = packs_root.join("messaging").join(pack.filename);
        let raw = fs::read_to_string(&path)?;
        out.push(LoadedPack {
            id: pack.id.to_string(),
            path,
            raw,
        });
    }
    Ok(out)
}

#[cfg(test)]
mod tests {
    use super::*;
    use once_cell::sync::Lazy;
    use std::path::PathBuf;
    use std::sync::Mutex;

    static ENV_GUARD: Lazy<Mutex<()>> = Lazy::new(|| Mutex::new(()));

    struct GuardEnv;
    impl GuardEnv {
        fn new() -> Self {
            Self
        }
    }
    impl Drop for GuardEnv {
        fn drop(&mut self) {
            unsafe {
                env::remove_var("MESSAGING_ADAPTER_PACK_PATHS");
                env::remove_var("MESSAGING_INSTALL_ALL_DEFAULT_ADAPTER_PACKS");
                env::remove_var("MESSAGING_DEFAULT_ADAPTER_PACKS");
            }
        }
    }

    #[test]
    fn adapter_pack_paths_env_parses_list() {
        let _lock = ENV_GUARD.lock().unwrap();
        let _guard = GuardEnv::new();
        unsafe {
            env::set_var(
                "MESSAGING_ADAPTER_PACK_PATHS",
                "/tmp/one.yaml, /tmp/two.yaml ,",
            );
        }
        let paths = adapter_pack_paths_from_env();
        assert_eq!(
            paths,
            vec![
                PathBuf::from("/tmp/one.yaml"),
                PathBuf::from("/tmp/two.yaml")
            ]
        );
    }

    #[test]
    fn config_parses_env() {
        let _lock = ENV_GUARD.lock().unwrap();
        let _guard = GuardEnv::new();
        unsafe {
            env::set_var("MESSAGING_INSTALL_ALL_DEFAULT_ADAPTER_PACKS", "true");
            env::set_var("MESSAGING_DEFAULT_ADAPTER_PACKS", "teams, slack");
        }
        let cfg = DefaultAdapterPacksConfig::from_env();
        assert!(cfg.install_all);
        assert_eq!(cfg.selected, vec!["teams".to_string(), "slack".to_string()]);
    }

    #[test]
    fn resolve_all_when_flag_set() {
        let cfg = DefaultAdapterPacksConfig {
            install_all: true,
            selected: vec![],
        };
        let resolved = resolve_default_adapter_packs(&cfg);
        assert_eq!(resolved.len(), DEFAULT_PACKS.len());
    }

    #[test]
    fn resolve_subset_when_listed() {
        let cfg = DefaultAdapterPacksConfig {
            install_all: false,
            selected: vec!["teams".into(), "slack".into(), "missing".into()],
        };
        let resolved = resolve_default_adapter_packs(&cfg);
        let ids: HashSet<&str> = resolved.iter().map(|p| p.id).collect();
        assert_eq!(ids, HashSet::from(["teams", "slack"]));
    }

    #[test]
    fn resolve_empty_when_no_selection() {
        let cfg = DefaultAdapterPacksConfig {
            install_all: false,
            selected: Vec::new(),
        };
        let resolved = resolve_default_adapter_packs(&cfg);
        assert!(resolved.is_empty());
    }

    #[test]
    fn resolve_all_when_install_all_true() {
        let cfg = DefaultAdapterPacksConfig {
            install_all: true,
            selected: Vec::new(),
        };
        let resolved = resolve_default_adapter_packs(&cfg);
        assert_eq!(resolved.len(), super::DEFAULT_PACKS.len());
    }

    #[test]
    fn resolve_exact_subset() {
        let cfg = DefaultAdapterPacksConfig {
            install_all: false,
            selected: vec!["slack".into(), "telegram".into()],
        };
        let resolved = resolve_default_adapter_packs(&cfg);
        let ids: Vec<_> = resolved.iter().map(|p| p.id).collect();
        assert_eq!(ids, vec!["slack", "telegram"]);
    }
}
