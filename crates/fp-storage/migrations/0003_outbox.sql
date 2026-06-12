-- 0003: the transactional outbox (spec/10 §3.3) — the event backbone replacing v1's
-- DB-polling integration. Events are appended in the SAME transaction as the mutation they
-- describe; consumers advance durable cursors under SKIP LOCKED (multi-replica safe).

CREATE TABLE events (
    seq           BIGSERIAL PRIMARY KEY,
    id            UUID NOT NULL UNIQUE,
    occurred_at   TIMESTAMPTZ NOT NULL DEFAULT now(),
    event_type    TEXT NOT NULL,
    org_id        UUID,
    team_id       UUID,
    payload       JSONB NOT NULL,
    -- W3C trace context of the originating request (spec/10 §8a): a config change is one
    -- trace from the API call through the async hop to the xDS push.
    trace_context JSONB NOT NULL DEFAULT '{}'::jsonb
);
CREATE INDEX idx_events_team ON events(team_id, seq);

CREATE TABLE event_cursors (
    consumer   TEXT PRIMARY KEY,
    last_seq   BIGINT NOT NULL DEFAULT 0,
    updated_at TIMESTAMPTZ NOT NULL DEFAULT now()
);

-- Wake sleeping consumers without polling latency; the poll fallback covers missed
-- notifications (LISTEN/NOTIFY is best-effort across reconnects).
CREATE FUNCTION fp_notify_event() RETURNS trigger AS $$
BEGIN
    PERFORM pg_notify('fp_events', NEW.seq::text);
    RETURN NULL;
END;
$$ LANGUAGE plpgsql;

CREATE TRIGGER events_notify AFTER INSERT ON events
    FOR EACH ROW EXECUTE FUNCTION fp_notify_event();
