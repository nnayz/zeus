import DirijorCore
import DirijorProtocol
import Foundation

/// Seq-stamped pub/sub with a bounded replay ring, backing `events.subscribe`
/// (gapless reconnect via sinceSeq, server-side filtering) and `events.wait`
/// (long-poll).
///
/// Backpressure is the load-bearing property here: the daemon is long-lived and
/// a subscriber may be a script that stopped reading, a laptop that went to
/// sleep mid-`ssh`, or a crashed app whose socket hasn't been reaped yet.
/// `publish` therefore never suspends and never waits on a consumer — each
/// subscriber owns a fixed-size queue, and when it overflows the *oldest*
/// queued events are evicted so the newest state still gets through. The
/// subscriber is told about the hole exactly once per burst via a synthetic
/// `events.dropped` marker, which is what makes the loss recoverable rather
/// than silent.
public actor EventBus {
    public struct Event: Sendable {
        public var name: String
        public var seq: UInt64
        /// The session this event is about, when it is about one. Kept out of
        /// `params` so filtering never has to decode the payload — a subscriber
        /// watching one session shouldn't cost a JSON parse per publish.
        public var sessionID: SessionID?
        fileprivate var storedParams: StoredParams

        public var params: JSONValue {
            get {
                switch storedParams {
                case .value(let value):
                    return value
                case .encoded(let bytes):
                    return (try? JSONDecoder.dirijor.decode(
                        JSONValue.self, from: Data(bytes))) ?? .null
                }
            }
            set { storedParams = .value(newValue) }
        }

        fileprivate init(name: String, seq: UInt64, sessionID: SessionID?, params: JSONValue) {
            self.name = name
            self.seq = seq
            self.sessionID = sessionID
            self.storedParams = .value(params)
        }

        fileprivate init(
            name: String, seq: UInt64, sessionID: SessionID?,
            encodedParams: ContiguousArray<UInt8>
        ) {
            self.name = name
            self.seq = seq
            self.sessionID = sessionID
            self.storedParams = .encoded(encodedParams)
        }

        fileprivate enum StoredParams: Sendable {
            case value(JSONValue)
            case encoded(ContiguousArray<UInt8>)
        }
    }

    /// Server-side subscription filter. Filtering here rather than in the
    /// connection handler means a narrow subscriber's queue only fills with
    /// events it asked for, so its bound actually protects it.
    public struct Filter: Sendable {
        public var sessions: Set<SessionID>?
        public var kinds: Set<String>?

        public init(sessions: Set<SessionID>? = nil, kinds: Set<String>? = nil) {
            self.sessions = sessions?.isEmpty == true ? nil : sessions
            self.kinds = kinds?.isEmpty == true ? nil : kinds
        }

        public static let all = Filter()

        func admits(_ event: Event) -> Bool {
            // The drop marker is the one thing a filter can never hide: a
            // subscriber that asked for a narrow slice still has to learn that
            // its slice has a hole.
            if event.name == EventName.eventsDropped { return true }
            if let kinds, !kinds.contains(event.name) { return false }
            if let sessions {
                guard let id = event.sessionID, sessions.contains(id) else { return false }
            }
            return true
        }
    }

    private struct ArchivedEvent: Sendable {
        var name: String
        var seq: UInt64
        var sessionID: SessionID?
        /// `Data` produced by Foundation's JSON encoder can keep a much larger
        /// malloc capacity than its logical byte count (tens of KiB for a few
        /// hundred bytes on current macOS). Copying once into native compact
        /// byte storage makes the ring's byte limit describe resident memory.
        var encodedParams: ContiguousArray<UInt8>

        var storageBytes: Int { name.utf8.count + encodedParams.count + 16 }
        var residentEstimateBytes: Int {
            name.utf8.count + encodedParams.capacity + MemoryLayout<Self>.stride
        }
    }

    /// One live subscription: its stream, its filter, and the bookkeeping for
    /// the drop marker it owes the consumer.
    private struct Subscriber {
        var continuation: AsyncStream<Event>.Continuation
        var filter: Filter
        var droppedCount = 0
        var firstDroppedSeq: UInt64 = 0
        var lastDroppedSeq: UInt64 = 0
    }

    private var nextSeq: UInt64 = 1
    /// Archived replay entries use encoded Data instead of retaining the much
    /// larger nested Dictionary/Array object graph behind JSONValue.
    private var ring: [ArchivedEvent?] = []
    private var ringStart = 0
    private var ringBytes = 0
    private let ringCapacity: Int
    private let ringByteCapacity: Int
    /// Per-subscriber queue bound. Defaults to twice the replay ring so a
    /// full `sinceSeq` replay — which lands before the consumer has read a
    /// single event — can never be the thing that triggers a drop.
    private let subscriberCapacity: Int
    private var subscribers: [UUID: Subscriber] = [:]

    public init(
        ringCapacity: Int = 4096,
        ringByteCapacity: Int = 8 * 1_024 * 1_024,
        subscriberCapacity: Int? = nil
    ) {
        self.ringCapacity = max(0, ringCapacity)
        self.ringByteCapacity = max(0, ringByteCapacity)
        self.subscriberCapacity = max(1, subscriberCapacity ?? (max(1, ringCapacity) * 2))
    }

    public func publish(name: String, params: JSONValue, sessionID: SessionID? = nil) {
        let event = Event(name: name, seq: nextSeq, sessionID: sessionID, params: params)
        nextSeq += 1
        archive(event)
        for id in subscribers.keys {
            deliver(event, to: id)
        }
    }

    /// Encodes and publishes a typed payload. Returns without publishing if the
    /// payload can't encode — an event that can't be serialized is a daemon bug,
    /// never a reason to fail the caller's mutation.
    public func publish<T: Encodable>(name: String, encoding value: T, sessionID: SessionID? = nil) {
        guard let params = try? JSONValue(encoding: value) else { return }
        publish(name: name, params: params, sessionID: sessionID)
    }

    private func archive(_ event: Event) {
        guard ringCapacity > 0, ringByteCapacity > 0,
            let encoded = try? JSONEncoder.dirijor.encode(event.params)
        else { return }

        let archived = ArchivedEvent(
            name: event.name, seq: event.seq, sessionID: event.sessionID,
            encodedParams: ContiguousArray(encoded))
        ring.append(archived)
        ringBytes += archived.storageBytes

        while ring.count - ringStart > ringCapacity || ringBytes > ringByteCapacity {
            if let oldest = ring[ringStart] {
                ringBytes -= oldest.storageBytes
                ring[ringStart] = nil // release encoded storage immediately
            }
            ringStart += 1
        }

        // Amortized compaction: eviction itself stays O(1), unlike repeatedly
        // shifting a 4,096-element Array with removeFirst().
        if ringStart == ring.count {
            ring.removeAll(keepingCapacity: true)
            ringStart = 0
        } else if ringStart >= 1_024, ringStart * 2 >= ring.count {
            ring.removeFirst(ringStart)
            ringStart = 0
        }
    }

    /// Hands one event to one subscriber, absorbing overflow.
    ///
    /// `.bufferingNewest` reports the element it *evicted* (the oldest queued
    /// one) while still accepting the new one, which is exactly the signal
    /// needed to describe the hole: the newest state always wins, and the
    /// evicted seq range is remembered. The marker is emitted on the first
    /// enqueue that succeeds without eviction — i.e. once the consumer has
    /// caught up — so a subscriber that is merely slow gets one summary line
    /// instead of a marker interleaved with every event it manages to read.
    private func deliver(_ event: Event, to id: UUID) {
        guard var subscriber = subscribers[id], subscriber.filter.admits(event) else { return }
        switch subscriber.continuation.yield(event) {
        case .enqueued:
            if subscriber.droppedCount > 0 {
                let marker = EventsDroppedEvent(
                    dropped: subscriber.droppedCount,
                    fromSeq: subscriber.firstDroppedSeq,
                    toSeq: subscriber.lastDroppedSeq)
                subscriber.droppedCount = 0
                subscribers[id] = subscriber
                // seq 0 is outside the published seq space (it starts at 1), so
                // a consumer tracking `lastSeq` for gapless resume can ignore
                // the marker without special-casing it.
                subscriber.continuation.yield(
                    Event(
                        name: EventName.eventsDropped, seq: 0, sessionID: nil,
                        params: (try? JSONValue(encoding: marker)) ?? .null))
                return
            }
        case .dropped(let evicted):
            if subscriber.droppedCount == 0 { subscriber.firstDroppedSeq = evicted.seq }
            subscriber.lastDroppedSeq = evicted.seq
            subscriber.droppedCount += 1
        case .terminated:
            subscribers.removeValue(forKey: id)
            return
        @unknown default:
            break
        }
        subscribers[id] = subscriber
    }

    /// Subscribes; events with seq > sinceSeq still in the ring are replayed
    /// first. `filter` applies to both the replay and the live tail.
    public func subscribe(sinceSeq: UInt64? = nil, filter: Filter = .all) -> AsyncStream<Event> {
        let id = UUID()
        let (stream, continuation) = AsyncStream.makeStream(
            of: Event.self, bufferingPolicy: .bufferingNewest(subscriberCapacity))
        if let sinceSeq {
            for case let archived? in ring[ringStart...] where archived.seq > sinceSeq {
                let event = Event(
                    name: archived.name,
                    seq: archived.seq,
                    sessionID: archived.sessionID,
                    encodedParams: archived.encodedParams)
                if filter.admits(event) { continuation.yield(event) }
            }
        }
        subscribers[id] = Subscriber(continuation: continuation, filter: filter)
        continuation.onTermination = { [weak self] _ in
            Task { await self?.removeSubscriber(id) }
        }
        return stream
    }

    private func removeSubscriber(_ id: UUID) {
        subscribers.removeValue(forKey: id)
    }

    public var currentSeq: UInt64 { nextSeq - 1 }
    var subscriberCount: Int { subscribers.count }
    var replayStorageBytes: Int { ringBytes }
    var replayResidentEstimateBytes: Int {
        ring[ringStart...].compactMap { $0 }.reduce(0) { $0 + $1.residentEstimateBytes }
    }
    var replayEventCount: Int { ring.count - ringStart }
}
