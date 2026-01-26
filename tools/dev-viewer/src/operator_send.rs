use serde::{Deserialize, Serialize};
use std::path::Path;
use std::process::Command;

#[derive(Clone, Debug, Deserialize)]
pub struct OperatorSendRequest {
    pub provider_id: String,
    pub provider_type: String,
    pub pack_root: String,
    pub tenant: String,
    pub team: String,
    pub text: Option<String>,
    pub payload: String,
    #[serde(default)]
    pub dry_run: bool,
    #[serde(default)]
    pub extra_args: Vec<String>,
}

#[derive(Clone, Debug, Serialize)]
pub struct OperatorSendResult {
    pub ok: bool,
    pub exit_code: Option<i32>,
    pub stdout: String,
    pub stderr: String,
    pub command: Vec<String>,
}

pub fn run_operator_send(
    bin: &Path,
    request: &OperatorSendRequest,
) -> Result<OperatorSendResult, String> {
    let mut args = vec![
        "demo".to_string(),
        "send".to_string(),
        "--bundle".to_string(),
        request.pack_root.clone(),
        "--provider".to_string(),
        request.provider_type.clone(),
        "--args-json".to_string(),
        request.payload.clone(),
        "--tenant".to_string(),
        request.tenant.clone(),
        "--team".to_string(),
        request.team.clone(),
    ];
    if request.dry_run {
        args.push("--debug".to_string());
    }
    if let Some(text) = &request.text
        && !text.is_empty()
    {
        args.push("--text".to_string());
        args.push(text.clone());
    }
    args.extend(request.extra_args.clone());

    let command = Command::new(bin)
        .args(&args)
        .output()
        .map_err(|err| format!("failed to run operator binary ({}): {err}", bin.display()))?;
    let mut command_line = Vec::with_capacity(args.len() + 1);
    command_line.push(bin.display().to_string());
    command_line.extend(args);
    let stdout = String::from_utf8_lossy(&command.stdout).to_string();
    let stderr = String::from_utf8_lossy(&command.stderr).to_string();
    Ok(OperatorSendResult {
        ok: command.status.success(),
        exit_code: command.status.code(),
        stdout,
        stderr,
        command: command_line,
    })
}
