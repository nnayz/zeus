#![allow(dead_code)] // Shared by multiple independent conformance binaries and the smoke example.
use std::{
    os::unix::{fs::DirBuilderExt, net::UnixStream},
    sync::{
        Arc, Mutex,
        atomic::{AtomicBool, Ordering},
    },
    time::Duration,
};
use tokio::sync::watch;
use zeus_client::DaemonClient;
use zeus_companion::{
    auth::{AuthStore, now_ms},
    config::Config,
    server,
};
use zeus_companion_api::{PairRequest, PairResponse, Scope};
use zeus_companion_client::Client;
use zeus_engine::{control::ControlServer, detect::ManifestEngine, registry::Registry};

pub struct Fixture {
    pub temp: tempfile::TempDir,
    pub auth: AuthStore,
    pub origin: String,
    pub client: Client,
    pub credentials: PairResponse,
    pub daemon: Arc<DaemonClient>,
    pub registry: Arc<Mutex<Registry>>,
    pub engine: Arc<Mutex<Arc<ControlServer>>>,
    stop: Arc<AtomicBool>,
    sockets: Arc<Mutex<Vec<UnixStream>>>,
    acceptor: Option<std::thread::JoinHandle<()>>,
    shutdown: watch::Sender<bool>,
    gateway: Option<tokio::task::JoinHandle<std::io::Result<()>>>,
    config: Config,
    retired_engines: Mutex<Vec<Arc<ControlServer>>>,
    watcher: Option<std::thread::JoinHandle<()>>,
}
impl Fixture {
    pub async fn new() -> Self {
        let temp = tempfile::tempdir_in(std::fs::canonicalize("/tmp").unwrap()).unwrap();
        let manifests = ManifestEngine::load_dir(&zeus_engine::detect::bundled_manifest_dir())
            .unwrap()
            .0;
        let registry = Arc::new(Mutex::new(Registry::new(
            Arc::new(manifests),
            temp.path().join("state.json"),
        )));
        let initial = Arc::new(ControlServer::new(
            registry.clone(),
            temp.path().join("daemon.sock"),
        ));
        let listener = initial.bind().unwrap();
        listener.set_nonblocking(true).unwrap();
        let engine = Arc::new(Mutex::new(initial));
        let stop = Arc::new(AtomicBool::new(false));
        let watcher = zeus_engine::events::spawn_registry_watcher(
            registry.clone(),
            engine.lock().unwrap().events(),
            stop.clone(),
        );
        let sockets = Arc::new(Mutex::new(Vec::new()));
        let acceptor = {
            let stop = stop.clone();
            let engine = engine.clone();
            let sockets = sockets.clone();
            std::thread::spawn(move || {
                let mut threads = Vec::new();
                while !stop.load(Ordering::Acquire) {
                    match listener.accept() {
                        Ok((socket, _)) => {
                            socket.set_nonblocking(false).unwrap();
                            sockets.lock().unwrap().push(socket.try_clone().unwrap());
                            let engine = engine.lock().unwrap().clone();
                            threads.push(std::thread::spawn(move || {
                                let _ = engine.serve(socket);
                            }));
                        }
                        Err(e) if e.kind() == std::io::ErrorKind::WouldBlock => {
                            std::thread::sleep(Duration::from_millis(2))
                        }
                        Err(_) => break,
                    }
                }
                for socket in sockets.lock().unwrap().iter() {
                    let _ = socket.shutdown(std::net::Shutdown::Both);
                }
                for thread in threads {
                    let _ = thread.join();
                }
            })
        };
        let directory = temp.path().join("auth");
        std::fs::DirBuilder::new()
            .mode(0o700)
            .create(&directory)
            .unwrap();
        let auth = AuthStore { directory };
        auth.initialize().unwrap();
        let daemon = Arc::new(DaemonClient::for_companion(temp.path().join("daemon.sock")));
        daemon.connect();
        daemon
            .wait_until_connected(Duration::from_secs(5))
            .await
            .unwrap();
        daemon
            .companion_hello()
            .await
            .expect("Engine projection handshake");
        let listener = server::bind(&Config {
            bind: "127.0.0.1:0".parse().unwrap(),
            ..Config::default()
        })
        .await
        .unwrap();
        let address = listener.local_addr().unwrap();
        let origin = format!("http://{address}");
        let config = Config {
            bind: address,
            origins: vec![origin.clone()],
            ..Config::default()
        };
        let (shutdown, rx) = watch::channel(false);
        let gateway = tokio::spawn(server::serve(
            listener,
            config.clone(),
            auth.clone(),
            daemon.clone(),
            rx,
        ));
        let code = auth
            .enroll(
                vec![Scope::Read, Scope::Interact, Scope::Lifecycle],
                now_ms(),
            )
            .unwrap();
        let credentials = Client::pair(
            &origin,
            &PairRequest {
                api_major: 1,
                expected_server_id: auth.server_id().unwrap(),
                code,
                device_name: "fixture device".into(),
            },
        )
        .await
        .unwrap();
        let client = Client::new(
            &origin,
            credentials.token.clone(),
            credentials.server_id.clone(),
        )
        .unwrap();
        client
            .hello(&["sessions", "screen", "events", "control_lease", "send_text"])
            .await
            .unwrap();
        Self {
            temp,
            auth,
            origin,
            client,
            credentials,
            daemon,
            registry,
            engine,
            stop,
            sockets,
            acceptor: Some(acceptor),
            shutdown,
            gateway: Some(gateway),
            config,
            retired_engines: Mutex::new(Vec::new()),
            watcher: Some(watcher),
        }
    }
    pub async fn spawn_echo(&self) -> String {
        let result=self.daemon.request("session.spawn",Some(&serde_json::json!({"kind":{"shell":{}},"cwd":self.temp.path(),"argv":["/bin/sh","-c","stty -echo; printf 'fixture-ready\\n'; exec /bin/cat"],"title":"fixture","initialCols":80,"initialRows":24})),Some(Duration::from_secs(5))).await.unwrap();
        let id = result["id"].as_str().unwrap().to_owned();
        let deadline = tokio::time::Instant::now() + Duration::from_secs(5);
        loop {
            if self
                .client
                .screen(&id)
                .await
                .is_ok_and(|s| s.text.contains("fixture-ready"))
            {
                break;
            }
            assert!(tokio::time::Instant::now() < deadline);
            tokio::time::sleep(Duration::from_millis(10)).await;
        }
        id
    }
    pub async fn spawn_shell(&self) -> String {
        let cwd = std::env::var("HOME").unwrap_or_else(|_| self.temp.path().display().to_string());
        let result = self
            .daemon
            .request(
                "session.spawn",
                Some(&serde_json::json!({
                    "kind": {"shell": {}},
                    "cwd": cwd,
                    "argv": ["/bin/sh", "-c", "printf 'fixture-ready\n'; exec /bin/zsh -l"],
                    "title": "zsh",
                    "initialCols": 80,
                    "initialRows": 24
                })),
                Some(Duration::from_secs(5)),
            )
            .await
            .unwrap();
        let id = result["id"].as_str().unwrap().to_owned();
        let deadline = tokio::time::Instant::now() + Duration::from_secs(5);
        loop {
            if self
                .client
                .screen(&id)
                .await
                .is_ok_and(|s| s.text.contains("fixture-ready"))
            {
                break;
            }
            assert!(tokio::time::Instant::now() < deadline);
            tokio::time::sleep(Duration::from_millis(10)).await;
        }
        id
    }
    pub async fn stop_gateway(&mut self) {
        self.shutdown.send_replace(true);
        if let Some(task) = self.gateway.take() {
            tokio::time::timeout(Duration::from_secs(3), task)
                .await
                .unwrap()
                .unwrap()
                .unwrap();
        }
    }
    pub async fn restart_gateway(&mut self) {
        self.stop_gateway().await;
        let listener = server::bind(&self.config).await.unwrap();
        let (shutdown, rx) = watch::channel(false);
        self.shutdown = shutdown;
        self.gateway = Some(tokio::spawn(server::serve(
            listener,
            self.config.clone(),
            self.auth.clone(),
            self.daemon.clone(),
            rx,
        )));
    }
    pub async fn restart_engine_endpoint(&self) {
        let next = Arc::new(ControlServer::new(
            self.registry.clone(),
            self.temp.path().join("daemon.sock"),
        ));
        // The real Engine removes its socket on Drop. Keep the retired fixture
        // endpoint alive while the injected listener continues accepting.
        let old = std::mem::replace(&mut *self.engine.lock().unwrap(), next);
        self.retired_engines.lock().unwrap().push(old);
        for socket in self.sockets.lock().unwrap().drain(..) {
            let _ = socket.shutdown(std::net::Shutdown::Both);
        }
        let mut state = self.daemon.connection_state();
        let _ = tokio::time::timeout(Duration::from_secs(2), state.changed()).await;
        self.daemon
            .wait_until_connected(Duration::from_secs(5))
            .await
            .unwrap();
    }
}
impl Drop for Fixture {
    fn drop(&mut self) {
        self.shutdown.send_replace(true);
        if let Some(task) = self.gateway.take() {
            task.abort();
        }
        self.stop.store(true, Ordering::Release);
        if let Some(watcher) = self.watcher.take() {
            let _ = watcher.join();
        }
        for socket in self.sockets.lock().unwrap().iter() {
            let _ = socket.shutdown(std::net::Shutdown::Both);
        }
        if let Some(thread) = self.acceptor.take() {
            let _ = thread.join();
        }
        let mut registry = self.registry.lock().unwrap();
        for record in registry.records() {
            let _ = registry.terminate(&record.id.0, Duration::from_millis(100));
        }
    }
}
