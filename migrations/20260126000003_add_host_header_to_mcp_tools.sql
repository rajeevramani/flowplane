-- Add host_header column to mcp_tools table
-- This stores the virtual host domain to use as the Host header when executing API tools
-- Required for proper routing through Envoy to upstream services that validate Host headers

ALTER TABLE mcp_tools ADD COLUMN host_header TEXT;

-- Add comment explaining the column purpose
-- The host_header is derived from the virtual host's non-wildcard domain during tool generation
-- Example: "api.example.com" or "eos2gda7yd5h5nx.m.pipedream.net"
