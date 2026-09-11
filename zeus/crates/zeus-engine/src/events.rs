//! Seq-stamped pub/sub with a bounded replay ring, backing `events.subscribe`
//! (gapless reconnect via `sinceSeq`, server-side filtering) and `events.wait`
//! (long-poll).
//!
//! Ported from the Swift `EventBus` actor. Backpressure is the load-bearing
//! property: the daemon is long-lived, and a subscriber may be a script that
//! stopped reading, a laptop that slept mid-`ssh`, or a crashed app whose
//! socket hasn't been reaped. `publish` therefore never blocks on a consumer —
//! each subscriber owns a fixed-size queue, and on overflow the *oldest*
//! queued events are evicted so the newest state still gets through. The
//! subscriber learns about the hole exactly once per burst via a synthetic
//! `events.dropped` marker, which makes the loss recoverable rather than
//! silent.

use std::collections::{HashMap, HashSet, VecDeque};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Condvar, Mutex};
use std::time::{Duration, Instant};

use serde_json::json;
use zeus_proto::JsonValue;

/// The synthetic hole marker. Its seq is 0 — outside the published seq space,
/// which starts at 1 — so a consumer tracking `lastSeq` for gapless resume
/// can ignore it without special-casing.
pub const EVENTS_DROPPED: &str = "events.dropped";

/// One published event, as a subscriber receives it.
#[derive(Clone, Debug)]
pub struct Event {
    pub name: String,
    pub seq: u64,
    /// The session this event is about, when it is about one. Kept out of
    /// `params` so filtering never costs a JSON decode per publish.
    pub session_id: Option<String>,
    pub params: JsonValue,
}

/// Server-side subscription filter. Filtering here rather than at the
/// connection means a narrow subscriber's queue only fills with events it
/// asked for, so its bound actually protects it.
#[derive(Clone, Debug, Default)]
pub struct Filter {
    pub sessions: Option<HashSet<String>>,
    pub kinds: Option<HashSet<String>>,
}

impl Filter {
    pub fn all() -> Self {
        Self::default()
    }

    pub fn new(sessions: Option<Vec<String>>, kinds: Option<Vec<String>>) -> Self {
        let normalize = |list: Option<Vec<String>>| {
            list.map(HashSet::from_iter)
                .filter(|set: &HashSet<String>| !set.is_empty())
        };
        Self {
            sessions: normalize(sessions),
            kinds: normalize(kinds),
        }
    }

    fn admits(&self, event: &Event) -> bool {
        // The drop marker is the one thing a filter can never hide: a narrow
        // subscriber still has to learn its slice has a hole.
        if event.name == EVENTS_DROPPED {
            return true;
        }
        if let Some(kinds) = &self.kinds
            && !kinds.contains(&event.name)
        {
            return false;
        }
        if let Some(sessions) = &self.sessions {
            match &event.session_id {
                Some(id) if sessions.contains(id) => {}
                _ => return false,
            }
        }
        true
    }
}

/// A replay entry keeps the encoded params rather than the JSON object graph,
/// so the ring's byte bound describes resident memory.
struct Archived {
    name: String,
    seq: u64,
    session_id: Option<String>,
    params: Vec<u8>,
}

impl Archived {
    fn storage_bytes(&self) -> usize {
        self.name.len() + self.params.len() + 16
    }

    fn event(&self) -> Event {
        Event {
            name: self.name.clone(),
            seq: self.seq,
            session_id: self.session_id.clone(),
            params: serde_json::from_slice(&self.params).unwrap_or(JsonValue::Null),
        }
    }
}

/// One live subscription's queue, shared between the bus and its stream.
struct SubscriberQueue {
    state: Mutex<QueueState>,
    ready: Condvar,
}

struct QueueState {
    queue: VecDeque<Event>,
    filter: Filter,
    capacity: usize,
    dropped: u64,
    first_dropped_seq: u64,
    last_dropped_seq: u64,
    closed: bool,
    projection: bool,
}

impl SubscriberQueue {
    /// Enqueues without ever blocking the publisher. On overflow the oldest
    /// queued event is evicted and the hole remembered; the marker is emitted
    /// on the first enqueue that succeeds without eviction — once the
    /// consumer has caught up — so a merely-slow subscriber gets one summary
    /// line instead of a marker interleaved with every event it reads.
    fn push(&self, event: &Event) {
        let mut state = self.state.lock().expect("queue");
        if state.closed || !state.filter.admits(event) {
            return;
        }
        if state.queue.len() >= state.capacity {
            if let Some(evicted) = state.queue.pop_front() {
                // A queued recovery marker can itself be displaced by a new
                // burst. Preserve its hole instead of counting sentinel seq 0.
                let (count, first, last) = if evicted.name == EVENTS_DROPPED {
                    (
                        evicted.params["dropped"].as_u64().unwrap_or(0),
                        evicted.params["fromSeq"].as_u64().unwrap_or(0),
                        evicted.params["toSeq"].as_u64().unwrap_or(0),
                    )
                } else {
                    (1, evicted.seq, evicted.seq)
                };
                if state.dropped == 0 {
                    state.first_dropped_seq = first;
                }
                state.last_dropped_seq = last;
                state.dropped = state.dropped.saturating_add(count);
            }
        } else if state.dropped > 0 && state.queue.len() + 2 <= state.capacity {
            let marker = Event {
                name: EVENTS_DROPPED.into(),
                seq: 0,
                session_id: None,
                params: json!({
                    "dropped": state.dropped,
                    "fromSeq": state.first_dropped_seq,
                    "toSeq": state.last_dropped_seq,
                }),
            };
            state.dropped = 0;
            state.queue.push_back(marker);
        }
        if state.projection {
            state.queue.push_back(Event {
                name: "companion.changed".into(),
                seq: event.seq,
                session_id: None,
                params: JsonValue::Null,
            });
        } else {
            state.queue.push_back(event.clone());
        }
        drop(state);
        self.ready.notify_all();
    }
}

struct BusInner {
    next_seq: u64,
    ring: VecDeque<Archived>,
    ring_bytes: usize,
    subscribers: HashMap<u64, Arc<SubscriberQueue>>,
    next_subscriber: u64,
    companion_subscribers: usize,
}

/// The bus itself; cheap to clone, shared by the control server and the
/// registry watcher.
#[derive(Clone)]
pub struct EventBus {
    inner: Arc<Mutex<BusInner>>,
    ring_capacity: usize,
    ring_byte_capacity: usize,
    subscriber_capacity: usize,
}

impl Default for EventBus {
    fn default() -> Self {
        Self::new()
    }
}

impl EventBus {
    pub fn new() -> Self {
        Self::with_capacities(4096, 8 << 20, None)
    }

    /// `subscriber_capacity` defaults to twice the ring, so a full `sinceSeq`
    /// replay — which lands before the consumer reads a single event — can
    /// never itself trigger a drop.
    pub fn with_capacities(
        ring_capacity: usize,
        ring_byte_capacity: usize,
        subscriber_capacity: Option<usize>,
    ) -> Self {
        Self {
            inner: Arc::new(Mutex::new(BusInner {
                next_seq: 1,
                ring: VecDeque::new(),
                ring_bytes: 0,
                subscribers: HashMap::new(),
                next_subscriber: 0,
                companion_subscribers: 0,
            })),
            ring_capacity,
            ring_byte_capacity,
            subscriber_capacity: subscriber_capacity
                .unwrap_or(ring_capacity.max(1) * 2)
                // A recovery marker and the newest event must fit together.
                .max(2),
        }
    }

    pub fn publish(&self, name: &str, params: JsonValue, session_id: Option<&str>) {
        let mut inner = self.inner.lock().expect("bus");
        let event = Event {
            name: name.to_string(),
            seq: inner.next_seq,
            session_id: session_id.map(str::to_string),
            params,
        };
        inner.next_seq += 1;

        if self.ring_capacity > 0
            && self.ring_byte_capacity > 0
            && let Ok(encoded) = serde_json::to_vec(&event.params)
        {
            let archived = Archived {
                name: event.name.clone(),
                seq: event.seq,
                session_id: event.session_id.clone(),
                params: encoded,
            };
            inner.ring_bytes += archived.storage_bytes();
            inner.ring.push_back(archived);
            while inner.ring.len() > self.ring_capacity
                || inner.ring_bytes > self.ring_byte_capacity
            {
                if let Some(oldest) = inner.ring.pop_front() {
                    inner.ring_bytes -= oldest.storage_bytes();
                } else {
                    break;
                }
            }
        }

        let queues: Vec<Arc<SubscriberQueue>> = inner.subscribers.values().cloned().collect();
        drop(inner);
        for queue in queues {
            queue.push(&event);
        }
    }

    /// Encodes and publishes a typed payload. An event that cannot serialize
    /// is a daemon bug, never a reason to fail the caller's mutation.
    pub fn publish_encoded<T: serde::Serialize>(
        &self,
        name: &str,
        value: &T,
        session_id: Option<&str>,
    ) {
        if let Ok(params) = serde_json::to_value(value) {
            self.publish(name, params, session_id);
        }
    }

    /// Subscribes; ring events with `seq > since_seq` are replayed first.
    /// The filter applies to both the replay and the live tail.
    pub fn subscribe(&self, since_seq: Option<u64>, filter: Filter) -> EventStream {
        self.subscribe_projected(since_seq, filter, false)
    }

    /// A bounded invalidation queue, with payloads removed before allocation.
    pub fn subscribe_companion(&self) -> EventStream {
        self.subscribe_projected(
            None,
            Filter::new(
                None,
                Some(vec![
                    "session.updated".into(),
                    "session.removed".into(),
                    "project.updated".into(),
                    "session.output".into(),
                    "terminal.control_changed".into(),
                ]),
            ),
            true,
        )
    }

    fn subscribe_projected(
        &self,
        since_seq: Option<u64>,
        filter: Filter,
        projection: bool,
    ) -> EventStream {
        let queue = Arc::new(SubscriberQueue {
            state: Mutex::new(QueueState {
                queue: VecDeque::new(),
                filter,
                capacity: if projection {
                    16
                } else {
                    self.subscriber_capacity
                },
                dropped: 0,
                first_dropped_seq: 0,
                last_dropped_seq: 0,
                closed: false,
                projection,
            }),
            ready: Condvar::new(),
        });

        let mut inner = self.inner.lock().expect("bus");
        if let Some(since) = since_seq {
            for archived in inner.ring.iter().filter(|archived| archived.seq > since) {
                queue.push(&archived.event());
            }
        }
        let id = inner.next_subscriber;
        inner.next_subscriber += 1;
        inner.subscribers.insert(id, Arc::clone(&queue));
        if projection {
            inner.companion_subscribers += 1;
        }
        EventStream {
            bus: Arc::clone(&self.inner),
            id,
            queue,
            projection,
        }
    }

    fn has_companion_subscribers(&self) -> bool {
        self.inner.lock().expect("bus").companion_subscribers > 0
    }

    pub fn current_seq(&self) -> u64 {
        self.inner.lock().expect("bus").next_seq - 1
    }

    #[cfg(test)]
    fn subscriber_count(&self) -> usize {
        self.inner.lock().expect("bus").subscribers.len()
    }
}

/// The receiving half of a subscription; dropping it unsubscribes.
pub struct EventStream {
    bus: Arc<Mutex<BusInner>>,
    id: u64,
    queue: Arc<SubscriberQueue>,
    projection: bool,
}

impl EventStream {
    /// Blocks until an event arrives or `timeout` elapses.
    pub fn recv(&self, timeout: Duration) -> Option<Event> {
        let deadline = Instant::now() + timeout;
        let mut state = self.queue.state.lock().expect("queue");
        loop {
            if let Some(event) = state.queue.pop_front() {
                return Some(event);
            }
            let remaining = deadline.checked_duration_since(Instant::now())?;
            let (next, wait) = self
                .queue
                .ready
                .wait_timeout(state, remaining)
                .expect("queue");
            state = next;
            if wait.timed_out() && state.queue.is_empty() {
                return None;
            }
        }
    }

    /// An event already queued, without waiting.
    pub fn try_recv(&self) -> Option<Event> {
        self.queue.state.lock().expect("queue").queue.pop_front()
    }
}

impl Drop for EventStream {
    fn drop(&mut self) {
        self.queue.state.lock().expect("queue").closed = true;
        if let Ok(mut inner) = self.bus.lock() {
            inner.subscribers.remove(&self.id);
            if self.projection {
                inner.companion_subscribers -= 1;
            }
        }
    }
}

/// Whether `status` satisfies an `events.wait` target. The alias table
/// ("done" ⇒ idle, "needs_me"/"needs-input"/"blocked" ⇒ needsInput) is the
/// Swift daemon's, so every caller resolves the same vocabulary.
pub fn satisfies_wait_target(status: &zeus_proto::SessionStatus, target: &str) -> bool {
    use zeus_proto::SessionStatus as S;
    match target {
        "idle" | "done" => matches!(status, S::Idle),
        "working" => matches!(status, S::Working),
        "starting" => matches!(status, S::Starting),
        "unknown" => matches!(status, S::Unknown),
        "needsInput" | "needs_input" | "needs-input" | "needs_me" | "blocked" => {
            matches!(status, S::NeedsInput(_))
        }
        "exited" | "dead" => matches!(status, S::Exited(_)),
        _ => false,
    }
}

/// Publishes `session.updated` whenever a live session's observable state
/// changes, by diffing registry views on a short cadence. The Swift daemon
/// publishes at each mutation site inside its status engine; this engine's
/// state changes on pump threads, so a watcher is the equivalent seam.
pub fn spawn_registry_watcher(
    registry: Arc<Mutex<crate::registry::Registry>>,
    events: EventBus,
    stop: Arc<AtomicBool>,
) -> std::thread::JoinHandle<()> {
    std::thread::Builder::new()
        .name("zeus-events-watcher".into())
        .spawn(move || {
            // Each session bumps a version counter exactly when its status,
            // needs-input, or title change, so the steady-state poll is one
            // integer compare per live session — the previous implementation
            // cloned and JSON-serialized every record (live and archived) on
            // every pass, all under the registry lock.
            let mut published: HashMap<String, u64> = HashMap::new();
            let mut screens = ScreenChanges::default();
            while !stop.load(Ordering::SeqCst) {
                let observe_screens = events.has_companion_subscribers();
                let (changed, screen_changed) = {
                    let Ok(mut registry) = registry.lock() else {
                        break;
                    };
                    let changed = registry.changed_since(&mut published);
                    let screen_changed = screens.sample(observe_screens, registry.grid_sources());
                    (changed, screen_changed)
                };
                for (id, record) in changed {
                    events.publish_encoded(
                        zeus_proto::EventName::SESSION_UPDATED,
                        &record,
                        Some(&id),
                    );
                }
                if screen_changed {
                    // Coalesce all screens into one payload-free invalidation.
                    // No terminal snapshot or diff is built by this watcher.
                    events.publish("session.output", JsonValue::Null, None);
                }
                std::thread::sleep(Duration::from_millis(150));
            }
        })
        .expect("spawn watcher")
}

/// Opt-in screen observation on the existing 150 ms registry cadence. The map
/// contains only live grid sources, never terminal data. PTY pumps are unchanged;
/// quiet sessions cost one small generation read and allocate nothing per tick.
#[derive(Default)]
struct ScreenChanges {
    observed: HashMap<String, ObservedScreen>,
}

struct ObservedScreen {
    source: crate::session::GridWake,
    generation: u64,
    present: bool,
}

impl ScreenChanges {
    fn sample<'a>(
        &mut self,
        enabled: bool,
        sources: impl Iterator<Item = (&'a str, crate::session::GridWake)>,
    ) -> bool {
        if !enabled {
            self.observed.clear();
            return false;
        }
        for previous in self.observed.values_mut() {
            previous.present = false;
        }
        let mut changed = false;
        for (id, source) in sources {
            let generation = source.generation();
            match self.observed.get_mut(id) {
                Some(previous) => {
                    changed |=
                        previous.generation != generation || !previous.source.same_source(&source);
                    previous.generation = generation;
                    previous.source = source;
                    previous.present = true;
                }
                None => {
                    changed = true;
                    self.observed.insert(
                        id.to_owned(),
                        ObservedScreen {
                            source,
                            generation,
                            present: true,
                        },
                    );
                }
            }
        }
        self.observed.retain(|_, previous| {
            changed |= !previous.present;
            previous.present
        });
        changed
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn event_names(stream: &EventStream) -> Vec<String> {
        let mut names = Vec::new();
        while let Some(event) = stream.try_recv() {
            names.push(event.name);
        }
        names
    }

    #[test]
    fn events_arrive_in_publish_order_with_increasing_seqs() {
        let bus = EventBus::new();
        let stream = bus.subscribe(None, Filter::all());
        bus.publish("a", json!({"n": 1}), None);
        bus.publish("b", json!({"n": 2}), None);

        let first = stream.recv(Duration::from_secs(1)).expect("first");
        let second = stream.recv(Duration::from_secs(1)).expect("second");
        assert_eq!((first.name.as_str(), first.seq), ("a", 1));
        assert_eq!((second.name.as_str(), second.seq), ("b", 2));
    }

    #[test]
    fn since_seq_replays_the_ring_gaplessly() {
        let bus = EventBus::new();
        bus.publish("one", json!({}), None);
        bus.publish("two", json!({}), None);
        bus.publish("three", json!({}), None);

        let stream = bus.subscribe(Some(1), Filter::all());
        assert_eq!(event_names(&stream), ["two", "three"]);
    }

    #[test]
    fn filters_narrow_by_kind_and_session() {
        let bus = EventBus::new();
        let stream = bus.subscribe(
            None,
            Filter::new(
                Some(vec!["s_1".into()]),
                Some(vec!["session.updated".into()]),
            ),
        );
        bus.publish("session.updated", json!({}), Some("s_1"));
        bus.publish("session.updated", json!({}), Some("s_2")); // other session
        bus.publish("worktree.created", json!({}), Some("s_1")); // other kind
        assert_eq!(event_names(&stream), ["session.updated"]);
    }

    #[test]
    fn overflow_evicts_oldest_and_marks_the_hole_once() {
        let bus = EventBus::with_capacities(64, 1 << 20, Some(2));
        let stream = bus.subscribe(None, Filter::all());
        for n in 0..5 {
            bus.publish("burst", json!({ "n": n }), None);
        }
        // Queue of 2: events 1..=3 were evicted; the newest two remain.
        let survivors: Vec<Event> = std::iter::from_fn(|| stream.try_recv()).collect();
        assert_eq!(survivors.len(), 2);
        assert_eq!(survivors[0].seq, 4);
        assert_eq!(survivors[1].seq, 5);

        // The consumer caught up: the next publish carries the marker first.
        bus.publish("after", json!({}), None);
        let marker = stream.recv(Duration::from_secs(1)).expect("marker");
        assert_eq!(marker.name, EVENTS_DROPPED);
        assert_eq!(marker.seq, 0, "outside the published seq space");
        assert_eq!(marker.params["dropped"], 3);
        assert_eq!(marker.params["fromSeq"], 1);
        assert_eq!(marker.params["toSeq"], 3);
        let after = stream.recv(Duration::from_secs(1)).expect("event");
        assert_eq!(after.name, "after");
    }

    #[test]
    fn a_dropped_stream_unsubscribes() {
        let bus = EventBus::new();
        let stream = bus.subscribe(None, Filter::all());
        assert_eq!(bus.subscriber_count(), 1);
        drop(stream);
        assert_eq!(bus.subscriber_count(), 0);
        bus.publish("into the void", json!({}), None); // must not panic
    }

    #[test]
    fn companion_invalidations_strip_payloads_and_include_control_changes() {
        let bus = EventBus::new();
        assert!(!bus.has_companion_subscribers());
        let stream = bus.subscribe_companion();
        assert!(bus.has_companion_subscribers());
        for name in [
            "session.output",
            "terminal.control_changed",
            "session.updated",
        ] {
            bus.publish(
                name,
                json!({"prompt": "sensitive", "token": "secret"}),
                Some("private"),
            );
            let event = stream.try_recv().expect("invalidation");
            assert_eq!(event.name, "companion.changed");
            assert!(event.params.is_null());
            assert!(event.session_id.is_none());
        }
        bus.publish("unrelated.internal", json!({}), None);
        assert!(stream.try_recv().is_none());
        drop(stream);
        assert!(!bus.has_companion_subscribers());
    }

    #[test]
    fn recovery_marker_never_exceeds_the_queue_bound() {
        let bus = EventBus::new();
        let stream = bus.subscribe_companion();
        for _ in 0..20 {
            bus.publish("session.output", JsonValue::Null, None);
        }
        assert_eq!(stream.queue.state.lock().unwrap().queue.len(), 16);
        stream.try_recv(); // One free slot cannot hold both marker and event.
        bus.publish("session.output", JsonValue::Null, None);
        assert_eq!(stream.queue.state.lock().unwrap().queue.len(), 16);
        stream.try_recv();
        stream.try_recv(); // Two slots can now hold marker and event.
        bus.publish("session.output", JsonValue::Null, None);
        let state = stream.queue.state.lock().unwrap();
        assert_eq!(state.queue.len(), 16);
        assert_eq!(
            state
                .queue
                .iter()
                .filter(|event| event.name == EVENTS_DROPPED)
                .count(),
            1
        );
    }

    #[test]
    fn evicted_recovery_marker_retains_its_original_hole() {
        let bus = EventBus::with_capacities(0, 0, Some(2));
        let stream = bus.subscribe(None, Filter::all());
        for _ in 0..3 {
            bus.publish("event", JsonValue::Null, None);
        }
        stream.try_recv();
        stream.try_recv();
        bus.publish("event", JsonValue::Null, None); // marker for seq 1, seq 4
        bus.publish("event", JsonValue::Null, None); // marker evicted
        stream.try_recv();
        stream.try_recv();
        bus.publish("event", JsonValue::Null, None);
        let marker = stream.try_recv().unwrap();
        assert_eq!(marker.name, EVENTS_DROPPED);
        assert_eq!(
            marker.params,
            json!({"dropped": 1, "fromSeq": 1, "toSeq": 1})
        );
    }

    #[test]
    fn output_invalidation_is_independent_of_status_and_reseeds_new_sources() {
        use crate::session::GridWake;
        let source = GridWake::new();
        let mut screens = ScreenChanges::default();
        let sample = |source: &GridWake| [("s_1", source.clone())].into_iter();
        assert!(screens.sample(true, sample(&source)));
        assert!(!screens.sample(true, sample(&source)));
        // The same signal used by local and remote PTY pumps; no status/title
        // change or SessionRecord clone is required for the view to refresh.
        source.notify();
        assert!(screens.sample(true, sample(&source)));
        assert!(!screens.sample(true, sample(&source)));
        let replacement = GridWake::new();
        replacement.notify(); // same generation, different session incarnation
        assert!(screens.sample(true, sample(&replacement)));
        assert!(screens.sample(true, std::iter::empty()));
        assert!(screens.observed.is_empty());
        assert!(screens.sample(true, sample(&source)));
        assert!(!screens.sample(
            false,
            std::iter::once_with(|| panic!("disabled source inspected"))
        ));
        assert!(screens.observed.is_empty());
        assert!(screens.sample(true, sample(&source)));
    }

    #[test]
    #[ignore = "manual registry screen-observation overhead measurement"]
    fn measure_companion_screen_observation_overhead() {
        let sources: Vec<_> = (0..1000)
            .map(|id| (format!("s_{id}"), crate::session::GridWake::new()))
            .collect();
        let mut screens = ScreenChanges::default();
        let mut samples = Vec::new();
        for _ in 0..1001 {
            let started = Instant::now();
            screens.sample(
                true,
                sources
                    .iter()
                    .map(|(id, source)| (id.as_str(), source.clone())),
            );
            samples.push(started.elapsed());
        }
        samples.remove(0); // Warm map: no per-tick allocation for quiet sessions.
        samples.sort_unstable();
        eprintln!(
            "1000 quiet Companion screen sources: p50={:?}, p90={:?}, p99={:?} per existing 150ms tick",
            samples[500], samples[900], samples[990]
        );
    }

    #[test]
    fn recv_times_out_when_nothing_is_published() {
        let bus = EventBus::new();
        let stream = bus.subscribe(None, Filter::all());
        let started = Instant::now();
        assert!(stream.recv(Duration::from_millis(50)).is_none());
        assert!(started.elapsed() >= Duration::from_millis(45));
    }

    #[test]
    fn wait_targets_resolve_the_swift_alias_table() {
        use zeus_proto::{ExitInfo, ExitReason, SessionStatus};
        let idle = SessionStatus::Idle;
        assert!(satisfies_wait_target(&idle, "idle"));
        assert!(satisfies_wait_target(&idle, "done"));
        assert!(!satisfies_wait_target(&idle, "working"));

        let exited = SessionStatus::Exited(ExitInfo {
            reason: ExitReason::Exited,
            code: Some(0),
            signal: None,
        });
        assert!(satisfies_wait_target(&exited, "exited"));
        assert!(satisfies_wait_target(&exited, "dead"));
        assert!(!satisfies_wait_target(&exited, "nonsense"));
    }
}
