-- Create listener_filters junction table for filter attachments
-- Allows attaching filters that support listener-level attachment (JwtAuth, RateLimit, ExtAuthz)
CREATE TABLE listener_filters (
    listener_id TEXT NOT NULL,
    filter_id TEXT NOT NULL,
    filter_order BIGINT NOT NULL,
    created_at TIMESTAMPTZ NOT NULL DEFAULT CURRENT_TIMESTAMP,

    FOREIGN KEY (listener_id) REFERENCES listeners(id) ON DELETE CASCADE,
    FOREIGN KEY (filter_id) REFERENCES filters(id) ON DELETE RESTRICT,

    PRIMARY KEY (listener_id, filter_id),
    UNIQUE(listener_id, filter_order)
);

CREATE INDEX idx_listener_filters_listener ON listener_filters(listener_id);
CREATE INDEX idx_listener_filters_filter ON listener_filters(filter_id);
