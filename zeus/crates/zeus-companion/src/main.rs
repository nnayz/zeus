use std::{
    io::{self, Read},
    path::PathBuf,
    sync::Arc,
    time::Duration,
};
use zeus_companion::{
    auth::{AuthStore, now_ms},
    config::{Config, atomic_write, invalid, secure_dir, secure_read},
    server,
};
use zeus_companion_api::Scope;

#[tokio::main]
async fn main() {
    if run().await.is_err() {
        eprintln!(
            "zeus-companion: operation failed (check configuration, ownership, and Engine availability)"
        );
        std::process::exit(1);
    }
}
async fn run() -> io::Result<()> {
    let args: Vec<_> = std::env::args_os().skip(1).collect();
    let action = args
        .first()
        .and_then(|s| s.to_str())
        .ok_or_else(|| invalid("missing operation"))?;
    match action {
        "init" if args.len() == 2 => {
            use std::os::unix::fs::DirBuilderExt;
            let directory = PathBuf::from(&args[1]);
            std::fs::DirBuilder::new().mode(0o700).create(&directory)?;
            secure_dir(&directory)?;
            AuthStore {
                directory: directory.clone(),
            }
            .initialize()?;
            atomic_write(
                &directory.join("config.json"),
                &serde_json::to_vec_pretty(&Config::default()).map_err(|_| invalid("config"))?,
            )
        }
        "enroll" if args.len() == 4 => {
            let config_path = PathBuf::from(&args[1]);
            let directory = config_path
                .parent()
                .ok_or_else(|| invalid("config path"))?
                .to_owned();
            let config: Config = serde_json::from_slice(&secure_read(&config_path, 16 * 1024)?)
                .map_err(|_| invalid("config"))?;
            config.validate()?;
            let scopes = match args[3].to_str() {
                Some("read") => vec![Scope::Read],
                Some("interact") => vec![Scope::Read, Scope::Interact],
                Some("lifecycle") => vec![Scope::Read, Scope::Interact, Scope::Lifecycle],
                _ => return Err(invalid("scope")),
            };
            let auth = AuthStore { directory };
            let now = now_ms();
            let code = auth.enroll(scopes, now)?;
            let payload = serde_json::json!({"server_id":auth.server_id()?,"code":code,"expires_at_ms":now+300_000});
            // Enrollment material only goes to an explicit owner-only file.
            atomic_write(
                &PathBuf::from(&args[2]),
                &serde_json::to_vec(&payload).map_err(|_| invalid("enrollment"))?,
            )
        }
        "revoke" if args.len() == 3 => AuthStore {
            directory: PathBuf::from(&args[1]),
        }
        .revoke(args[2].to_str().ok_or_else(|| invalid("device id"))?),
        "devices" if args.len() == 2 => {
            let devices = AuthStore {
                directory: PathBuf::from(&args[1]),
            }
            .list()?;
            for device in devices {
                println!(
                    "{} revoked={} expires_at_ms={}",
                    device.id, device.revoked, device.expires_at_ms
                );
            }
            Ok(())
        }
        "serve" if args.len() == 3 => {
            let config_path = PathBuf::from(&args[1]);
            let config: Config = serde_json::from_slice(&secure_read(&config_path, 16 * 1024)?)
                .map_err(|_| invalid("config"))?;
            let auth = AuthStore {
                directory: config_path
                    .parent()
                    .ok_or_else(|| invalid("config path"))?
                    .to_owned(),
            };
            auth.server_id()?;
            let client = Arc::new(zeus_client::DaemonClient::for_companion(PathBuf::from(
                &args[2],
            )));
            client.connect();
            client
                .wait_until_connected(Duration::from_secs(10))
                .await
                .map_err(|_| invalid("Engine unavailable"))?;
            client
                .companion_hello()
                .await
                .map_err(|_| invalid("Engine companion capability unavailable"))?;
            let listener = server::bind(&config).await?;
            let (shutdown, receiver) = tokio::sync::watch::channel(false);
            // The Engine owns this pipe. EOF on crash, shutdown or replacement
            // tears down the listener, clients and device-memory projections.
            std::thread::spawn(move || {
                let mut input = std::io::stdin();
                let mut byte = [0_u8; 1];
                let _ = input.read(&mut byte);
                shutdown.send_replace(true);
            });
            server::serve(listener, config, auth, client.clone(), receiver).await?;
            client.shutdown().await;
            Ok(())
        }
        _ => Err(invalid(
            "usage: init DIR | enroll CONFIG OUTPUT read|interact|lifecycle | devices DIR | revoke DIR DEVICE | serve CONFIG ENGINE_SOCKET",
        )),
    }
}
