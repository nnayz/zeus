//! Explicit opt-in sidecar lifecycle; no independent daemon or restart loop.
use std::{
    io,
    path::Path,
    process::{Child, ChildStdin, Command, Stdio},
};
pub struct CompanionSidecar {
    child: Child,
    _liveness: ChildStdin,
}
impl Drop for CompanionSidecar {
    fn drop(&mut self) {
        let _ = self.child.kill();
        let _ = self.child.wait();
    }
}
pub fn launch(exe_dir: &Path, socket: &Path) -> io::Result<Option<CompanionSidecar>> {
    let Some(config) = std::env::var_os("ZEUS_COMPANION_CONFIG") else {
        return Ok(None);
    };
    let executable = exe_dir.join("zeus-companion");
    let mut child = Command::new(executable)
        .arg("serve")
        .arg(config)
        .arg(socket)
        .env_clear()
        .stdin(Stdio::piped())
        .stdout(Stdio::null())
        .stderr(Stdio::inherit())
        .spawn()?;
    let liveness = child
        .stdin
        .take()
        .ok_or_else(|| io::Error::other("missing sidecar liveness pipe"))?;
    Ok(Some(CompanionSidecar {
        child,
        _liveness: liveness,
    }))
}
