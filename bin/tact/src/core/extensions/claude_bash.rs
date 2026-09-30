//! Workspace shell executor behind Claude's `Bash` tool.
//!
//! This is not a sandbox. Commands run as the user in the workspace directory, with the same
//! authority as the OpenAI backend's `exec_command` tool.

use nanocodex::tools::claude_bash::{BashRequest, BashResult, SandboxBashExecutor};
use std::{
    future::Future,
    path::PathBuf,
    process::{Command as StdCommand, Stdio},
    time::Duration,
};
use tokio::{
    io::{AsyncRead, AsyncReadExt},
    process::Command,
    time::timeout,
};

pub(crate) struct WorkspaceBash {
    workspace: PathBuf,
}

impl WorkspaceBash {
    pub(crate) const fn new(workspace: PathBuf) -> Self {
        Self { workspace }
    }
}

impl SandboxBashExecutor for WorkspaceBash {
    fn execute(
        &self,
        request: BashRequest,
    ) -> impl Future<Output = Result<BashResult, String>> + Send {
        let workspace = self.workspace.clone();
        async move {
            let mut child = Command::new("bash")
                .arg("-c")
                .arg(&request.command)
                .current_dir(&workspace)
                .stdin(Stdio::null())
                .stdout(Stdio::piped())
                .stderr(Stdio::piped())
                // Each command leads its own process group so a timeout can stop its descendants.
                .process_group(0)
                .kill_on_drop(true)
                .spawn()
                .map_err(|error| format!("failed to start bash: {error}"))?;
            let process_group = child.id();
            let stdout = child.stdout.take().expect("stdout is piped");
            let stderr = child.stderr.take().expect("stderr is piped");

            let completed = timeout(Duration::from_millis(request.timeout_ms), async {
                tokio::join!(
                    read_bounded(stdout, request.max_stdout_bytes),
                    read_bounded(stderr, request.max_stderr_bytes),
                    child.wait(),
                )
            })
            .await;
            let Ok((stdout, stderr, status)) = completed else {
                if let Some(process_group) = process_group {
                    kill_process_group(process_group);
                }
                return Err(format!("command timed out after {} ms", request.timeout_ms));
            };

            let (stdout, stdout_truncated) =
                stdout.map_err(|error| format!("failed to read stdout: {error}"))?;
            let (stderr, stderr_truncated) =
                stderr.map_err(|error| format!("failed to read stderr: {error}"))?;
            let status = status.map_err(|error| format!("failed to wait for bash: {error}"))?;
            Ok(BashResult {
                stdout,
                stderr,
                // A signal-terminated command has no exit code; report it like a shell would.
                exit_code: status.code().unwrap_or(-1),
                truncated: stdout_truncated || stderr_truncated,
            })
        }
    }
}

/// Keeps at most `limit` bytes while draining the rest, so a chatty command cannot block on a
/// full pipe or grow memory without bound.
async fn read_bounded(
    mut stream: impl AsyncRead + Unpin,
    limit: usize,
) -> std::io::Result<(String, bool)> {
    let mut kept = Vec::new();
    let mut truncated = false;
    let mut buffer = [0; 8192];
    loop {
        let read = stream.read(&mut buffer).await?;
        if read == 0 {
            break;
        }
        let room = limit.saturating_sub(kept.len());
        kept.extend_from_slice(&buffer[..read.min(room)]);
        truncated |= read > room;
    }
    Ok((String::from_utf8_lossy(&kept).into_owned(), truncated))
}

fn kill_process_group(process_group: u32) {
    drop(
        StdCommand::new("kill")
            .args(["-KILL", "--", &format!("-{process_group}")])
            .stdout(Stdio::null())
            .stderr(Stdio::null())
            .status(),
    );
}

#[cfg(test)]
mod tests {
    use super::WorkspaceBash;
    use nanocodex::tools::claude_bash::{BashRequest, SandboxBashExecutor};
    use tempfile::tempdir;

    fn request(command: &str, timeout_ms: u64, max_bytes: usize) -> BashRequest {
        BashRequest {
            command: command.to_owned(),
            description: None,
            timeout_ms,
            max_stdout_bytes: max_bytes,
            max_stderr_bytes: max_bytes,
        }
    }

    #[tokio::test]
    async fn runs_in_the_workspace_and_reports_output_and_status() {
        let workspace = tempdir().unwrap();
        let bash = WorkspaceBash::new(workspace.path().canonicalize().unwrap());

        let result = bash
            .execute(request("pwd; echo oops >&2; exit 3", 10_000, 4096))
            .await
            .unwrap();

        assert_eq!(
            result.stdout.trim(),
            workspace.path().canonicalize().unwrap().to_str().unwrap()
        );
        assert_eq!(result.stderr.trim(), "oops");
        assert_eq!(result.exit_code, 3);
        assert!(!result.truncated);
    }

    #[tokio::test]
    async fn bounds_captured_output() {
        let workspace = tempdir().unwrap();
        let bash = WorkspaceBash::new(workspace.path().to_path_buf());

        let result = bash
            .execute(request("head -c 100000 /dev/zero | tr '\\0' a", 10_000, 16))
            .await
            .unwrap();

        assert_eq!(result.stdout, "a".repeat(16));
        assert!(result.truncated);
    }

    #[tokio::test]
    async fn times_out_long_running_commands() {
        let workspace = tempdir().unwrap();
        let bash = WorkspaceBash::new(workspace.path().to_path_buf());

        let error = bash
            .execute(request("sleep 30", 100, 16))
            .await
            .unwrap_err();

        assert!(error.contains("timed out"), "{error}");
    }
}
