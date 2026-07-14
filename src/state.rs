use serde::Deserialize;
use std::path::{Path, PathBuf};

/// A single addressable resource instance from terraform state.
///
/// Matches one line of `terraform state list` output, e.g.:
///   "aws_vpc.main"
///   "null_resource.worker[0]"
///   "null_resource.zone[\"us-east-1\"]"
///   "module.net.module.subnets.null_resource.private[2]"
#[derive(Debug, Clone)]
pub struct StateResource {
    pub address: String,
    /// Full raw instance object (attributes, schema_version, sensitive_attributes, …).
    /// Used to power the resource detail view.
    pub instance: serde_json::Value,
}

impl StateResource {
    /// Returns true when the instance carries `"status": "tainted"`.
    pub fn is_tainted(&self) -> bool {
        self.instance.get("status").and_then(|v| v.as_str()) == Some("tainted")
    }
}

/// The result of reading a module's terraform state.
#[derive(Debug, Clone)]
pub enum StateContent {
    /// A background load is in flight; no content to show yet.
    Loading,
    /// Module has no `.terraform/` dir and no local state file.
    NotInitialized,
    /// Module is initialized but has no resources in state.
    NoState,
    /// Resources found in state.
    Resources(Vec<StateResource>),
    /// State could not be read or parsed; message is shown to the user.
    Error(String),
}

/// Read the terraform state for a module directory.
///
/// Initialization check: `.terraform/` must exist, OR a local `terraform.tfstate`
/// file must be present (handles modules whose `.terraform/` was cleaned up).
///
/// For remote backends (S3, GCS, Azure, Terraform Cloud, etc.) where no local
/// state file is available, runs `<binary> state pull` to fetch the state.
pub fn read_state(module_path: &Path, binary: &str) -> StateContent {
    let initialized = module_path.join(".terraform").exists();
    let local_tfstate = module_path.join("terraform.tfstate");

    if initialized {
        if let Some(path) = resolve_state_path(module_path) {
            return parse_state_content(&path);
        }
        // No local state file found — if this is a remote backend, pull it.
        if is_remote_backend(module_path) {
            return pull_remote_state(module_path, binary);
        }
        return StateContent::NoState;
    }

    // Not initialized via init, but has a local state file (legacy / CI cleanup).
    if local_tfstate.exists() {
        return parse_state_content(&local_tfstate);
    }

    StateContent::NotInitialized
}

/// Returns true if the module's initialized backend is not a local backend.
fn is_remote_backend(module_path: &Path) -> bool {
    let backend_meta = module_path.join(".terraform").join("terraform.tfstate");
    if let Ok(content) = std::fs::read_to_string(&backend_meta) {
        if let Ok(meta) = serde_json::from_str::<BackendMeta>(&content) {
            if let Some(backend) = meta.backend {
                return backend.backend_type != "local";
            }
        }
    }
    false
}

/// Fetch state from a remote backend by running `<binary> state pull`.
fn pull_remote_state(module_path: &Path, binary: &str) -> StateContent {
    let output = std::process::Command::new(binary)
        .args(["state", "pull"])
        .current_dir(module_path)
        .output();

    match output {
        Ok(out) if out.status.success() => {
            let json = String::from_utf8_lossy(&out.stdout);
            parse_state_from_str(&json)
        }
        Ok(out) => {
            let stderr = String::from_utf8_lossy(&out.stderr).trim().to_string();
            StateContent::Error(if stderr.is_empty() {
                "`state pull` failed".to_string()
            } else {
                stderr
            })
        }
        Err(e) => StateContent::Error(format!("failed to run `state pull`: {e}")),
    }
}

/// Resolve which state file to read for an initialized module.
///
/// Priority:
/// 1. Backend-configured local path from `.terraform/terraform.tfstate`
/// 2. `terraform.tfstate` in the module directory
pub(crate) fn resolve_state_path(module_path: &Path) -> Option<PathBuf> {
    let backend_meta = module_path.join(".terraform").join("terraform.tfstate");
    if let Ok(content) = std::fs::read_to_string(&backend_meta) {
        if let Ok(meta) = serde_json::from_str::<BackendMeta>(&content) {
            if let Some(backend) = meta.backend {
                if backend.backend_type == "local" {
                    if let Some(path_str) = backend.config.get("path").and_then(|v| v.as_str()) {
                        let resolved = normalize_path(&module_path.join(path_str));
                        if resolved.exists() {
                            return Some(resolved);
                        }
                    }
                }
            }
        }
    }

    let default = module_path.join("terraform.tfstate");
    if default.exists() {
        return Some(default);
    }

    None
}

/// Parse a state file and return one `StateResource` per instance.
fn parse_state_content(path: &Path) -> StateContent {
    match std::fs::read_to_string(path) {
        Ok(content) => parse_state_from_str(&content),
        Err(e) => StateContent::Error(format!("failed to read state file: {e}")),
    }
}

/// Parse terraform state JSON (from a file or `state pull` stdout).
///
/// count/for_each resources with N instances produce N addresses, matching
/// `terraform state list` output exactly.
fn parse_state_from_str(content: &str) -> StateContent {
    let state = match serde_json::from_str::<TfState>(content) {
        Ok(s) => s,
        Err(e) => return StateContent::Error(format!("failed to parse state: {e}")),
    };

    let resources: Vec<StateResource> = state
        .resources
        .unwrap_or_default()
        .into_iter()
        .flat_map(expand_resource)
        .collect();

    if resources.is_empty() {
        StateContent::NoState
    } else {
        StateContent::Resources(resources)
    }
}

/// Expand a resource block into one `StateResource` per instance.
///
/// Instances are stored as raw `serde_json::Value` so the detail view can
/// display the full body without re-reading the state file.
fn expand_resource(r: TfResource) -> Vec<StateResource> {
    let base = build_base_address(&r);

    // Single unindexed instance → plain resource, no suffix.
    if r.instances.len() == 1 {
        let inst = &r.instances[0];
        let has_index = inst.get("index_key").map(|v| !v.is_null()).unwrap_or(false);
        if !has_index {
            return vec![StateResource {
                address: base,
                instance: r.instances.into_iter().next().unwrap(),
            }];
        }
    }

    r.instances
        .into_iter()
        .map(|inst| {
            let suffix = match inst.get("index_key") {
                Some(serde_json::Value::Number(n)) => format!("[{}]", n),
                Some(serde_json::Value::String(s)) => format!("[\"{}\"]", s),
                Some(v) if !v.is_null() => format!("[{}]", v),
                _ => String::new(),
            };
            StateResource {
                address: format!("{}{}", base, suffix),
                instance: inst,
            }
        })
        .collect()
}

/// Build the base address for a resource block (without instance index suffix).
fn build_base_address(r: &TfResource) -> String {
    let leaf = if r.mode == "data" {
        format!("data.{}.{}", r.resource_type, r.name)
    } else {
        format!("{}.{}", r.resource_type, r.name)
    };
    match &r.module {
        Some(m) => format!("{}.{}", m, leaf),
        None => leaf,
    }
}

/// Resolve `..` components in a path without requiring it to exist on disk.
fn normalize_path(path: &Path) -> PathBuf {
    let mut out = PathBuf::new();
    for component in path.components() {
        match component {
            std::path::Component::ParentDir => {
                out.pop();
            }
            std::path::Component::CurDir => {}
            c => out.push(c),
        }
    }
    out
}

// ── Serde types ───────────────────────────────────────────────────────────────

#[derive(Deserialize)]
struct BackendMeta {
    backend: Option<BackendConfig>,
}

#[derive(Deserialize)]
struct BackendConfig {
    #[serde(rename = "type")]
    backend_type: String,
    config: serde_json::Value,
}

#[derive(Deserialize)]
struct TfState {
    resources: Option<Vec<TfResource>>,
}

#[derive(Deserialize)]
struct TfResource {
    mode: String,
    #[serde(rename = "type")]
    resource_type: String,
    name: String,
    module: Option<String>,
    /// Raw instance objects; each carries index_key, attributes, etc.
    #[serde(default)]
    instances: Vec<serde_json::Value>,
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    fn addresses(content: StateContent) -> Vec<String> {
        match content {
            StateContent::Resources(rs) => rs.into_iter().map(|r| r.address).collect(),
            other => panic!("expected Resources, got {other:?}"),
        }
    }

    #[test]
    fn corrupt_json_is_error_not_no_state() {
        let content = parse_state_from_str("{ not valid json");
        match content {
            StateContent::Error(_) => {}
            other => panic!("expected Error, got {other:?}"),
        }
    }

    #[test]
    fn empty_resources_is_no_state() {
        let content = parse_state_from_str(r#"{"resources": []}"#);
        assert!(matches!(content, StateContent::NoState));
    }

    #[test]
    fn unindexed_instance_has_no_suffix() {
        let r = TfResource {
            mode: "managed".to_string(),
            resource_type: "aws_vpc".to_string(),
            name: "main".to_string(),
            module: None,
            instances: vec![json!({"schema_version": 0})],
        };
        let addrs: Vec<String> = expand_resource(r).into_iter().map(|s| s.address).collect();
        assert_eq!(addrs, vec!["aws_vpc.main"]);
    }

    #[test]
    fn count_instances_get_numeric_index_suffix() {
        let r = TfResource {
            mode: "managed".to_string(),
            resource_type: "aws_instance".to_string(),
            name: "web".to_string(),
            module: None,
            instances: vec![
                json!({"index_key": 0}),
                json!({"index_key": 1}),
            ],
        };
        let addrs: Vec<String> = expand_resource(r).into_iter().map(|s| s.address).collect();
        assert_eq!(addrs, vec!["aws_instance.web[0]", "aws_instance.web[1]"]);
    }

    #[test]
    fn for_each_instance_gets_string_key_suffix() {
        let r = TfResource {
            mode: "managed".to_string(),
            resource_type: "null_resource".to_string(),
            name: "zone".to_string(),
            module: None,
            instances: vec![json!({"index_key": "us-east-1"})],
        };
        let addrs: Vec<String> = expand_resource(r).into_iter().map(|s| s.address).collect();
        assert_eq!(addrs, vec!["null_resource.zone[\"us-east-1\"]"]);
    }

    #[test]
    fn module_and_data_address_forms() {
        let data = TfResource {
            mode: "data".to_string(),
            resource_type: "aws_ami".to_string(),
            name: "ubuntu".to_string(),
            module: Some("module.net".to_string()),
            instances: vec![json!({})],
        };
        let addrs: Vec<String> = expand_resource(data).into_iter().map(|s| s.address).collect();
        assert_eq!(addrs, vec!["module.net.data.aws_ami.ubuntu"]);
    }

    #[test]
    fn parse_state_from_str_expands_multiple_resources() {
        let json_str = r#"{
            "resources": [
                {
                    "mode": "managed",
                    "type": "aws_vpc",
                    "name": "main",
                    "instances": [{"schema_version": 0}]
                }
            ]
        }"#;
        let addrs = addresses(parse_state_from_str(json_str));
        assert_eq!(addrs, vec!["aws_vpc.main"]);
    }
}
