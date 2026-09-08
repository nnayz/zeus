//! Read credentials on stdin; print only counts and timing, never session data.
use serde::Deserialize;
use std::io::Read;
#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct Input {
    origin: String,
    server_id: String,
    token: String,
}
#[tokio::main]
async fn main() -> Result<(), Box<dyn std::error::Error>> {
    let mut bytes = Vec::new();
    std::io::stdin().take(4097).read_to_end(&mut bytes)?;
    if bytes.len() > 4096 {
        return Err("credential input limit".into());
    }
    let config: Input = serde_json::from_slice(&bytes).map_err(|_| "invalid credential input")?;
    let client =
        zeus_companion_client::Client::new(&config.origin, config.token, config.server_id)?;
    client.hello(&["sessions", "events"]).await?;
    let mut events = client.subscribe(None).await?;
    let initial = tokio::time::timeout(std::time::Duration::from_secs(5), events.next()).await??;
    if initial.kind != "resync_required" {
        return Err("missing initial reseed".into());
    }
    let mut latency = Vec::new();
    let mut count = 0;
    for _ in 0..30 {
        let start = std::time::Instant::now();
        let page = client.sessions(&Default::default()).await?;
        count = page.items.len();
        latency.push(start.elapsed().as_micros());
    }
    latency.sort();
    println!(
        "requests=30 sessions_in_first_page={count} latency_p50_us={} latency_p90_us={} initial_reseed=ok",
        latency[14], latency[26]
    );
    Ok(())
}
