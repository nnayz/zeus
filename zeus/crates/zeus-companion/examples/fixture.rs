//! Isolated real Engine + loopback gateway for conformance clients.
//! The only argument is an owner-only enrollment output path, never a secret.
#[path = "../tests/support/mod.rs"]
mod support;
use std::io::{self, Read};
use zeus_companion::{
    auth::now_ms,
    config::{atomic_write, invalid},
};
use zeus_companion_api::Scope;

#[tokio::main]
async fn main() -> io::Result<()> {
    let output = std::env::args_os()
        .nth(1)
        .map(std::path::PathBuf::from)
        .ok_or_else(|| invalid("expected enrollment output path"))?;
    let fixture = support::Fixture::new().await;
    let _session = fixture.spawn_shell().await;
    let now = now_ms();
    let code = fixture
        .auth
        .enroll(vec![Scope::Read, Scope::Interact, Scope::Lifecycle], now)?;
    let payload = serde_json::json!({"origin":fixture.origin,"server_id":fixture.auth.server_id()?,"code":code,"expires_at_ms":now+300_000});
    atomic_write(
        &output,
        &serde_json::to_vec(&payload).map_err(|_| invalid("enrollment encoding"))?,
    )?;
    println!("{}", output.display());
    let (closed, wait) = tokio::sync::oneshot::channel();
    std::thread::spawn(move || {
        let mut byte = [0_u8; 1];
        let _ = std::io::stdin().read(&mut byte);
        let _ = closed.send(());
    });
    tokio::select! {_=wait=>{},_=tokio::signal::ctrl_c()=>{}}
    drop(fixture);
    let _ = std::fs::remove_file(output);
    Ok(())
}
