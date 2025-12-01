-- Create route_filters junction table for filter attachments
CREATE TABLE route_filters (
    route_id TEXT NOT NULL,
    filter_id TEXT NOT NULL,
    filter_order INTEGER NOT NULL,
    created_at DATETIME NOT NULL DEFAULT CURRENT_TIMESTAMP,

    FOREIGN KEY (route_id) REFERENCES routes(id) ON DELETE CASCADE,
    FOREIGN KEY (filter_id) REFERENCES filters(id) ON DELETE RESTRICT,

    PRIMARY KEY (route_id, filter_id),
    UNIQUE(route_id, filter_order)
);

CREATE INDEX idx_route_filters_route ON route_filters(route_id);
CREATE INDEX idx_route_filters_filter ON route_filters(filter_id);
