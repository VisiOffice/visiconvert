-- A site can run several number pools. Each pool replaces exactly one
-- published website number (its fallback_phone), like a CallRail swap target or
-- a WhatConverts swap number. A visit holds at most one number from each pool.
-- Existing single-pool sites keep their pool, numbers, assignments and versions.
ALTER TABLE dni_pool_numbers DROP CONSTRAINT dni_pool_numbers_workspace_id_site_id_fkey;
ALTER TABLE dni_pools DROP CONSTRAINT dni_pools_workspace_id_site_id_key;
ALTER TABLE dni_pools DROP CONSTRAINT dni_pools_pkey;

ALTER TABLE dni_pools ADD COLUMN id uuid NOT NULL DEFAULT gen_random_uuid();
ALTER TABLE dni_pools ADD COLUMN created_at timestamptz NOT NULL DEFAULT clock_timestamp();
ALTER TABLE dni_pools ADD PRIMARY KEY (id);
ALTER TABLE dni_pools ADD CONSTRAINT dni_pools_site_fallback_unique UNIQUE (site_id, fallback_phone);
ALTER TABLE dni_pools ADD CONSTRAINT dni_pools_scope_unique UNIQUE (workspace_id, site_id, id);
CREATE INDEX dni_pools_site ON dni_pools(workspace_id, site_id, created_at, id);

-- A number stays dedicated to one site. Within that site it belongs to one pool
-- at a time; it can move only after it is disabled in its previous pool.
ALTER TABLE dni_pool_numbers ADD COLUMN pool_id uuid;
UPDATE dni_pool_numbers n SET pool_id = p.id FROM dni_pools p
    WHERE p.workspace_id = n.workspace_id AND p.site_id = n.site_id;
ALTER TABLE dni_pool_numbers ALTER COLUMN pool_id SET NOT NULL;
ALTER TABLE dni_pool_numbers ADD CONSTRAINT dni_pool_numbers_pool_fkey
    FOREIGN KEY (workspace_id, site_id, pool_id) REFERENCES dni_pools(workspace_id, site_id, id);
CREATE INDEX dni_pool_numbers_pool ON dni_pool_numbers(pool_id) WHERE enabled;

-- The assignment records the pool it was leased from, so moving a number later
-- never changes which published number an earlier visit saw.
ALTER TABLE dni_assignments ADD COLUMN pool_id uuid;
UPDATE dni_assignments a SET pool_id = n.pool_id FROM dni_pool_numbers n
    WHERE n.number_id = a.number_id;
ALTER TABLE dni_assignments ALTER COLUMN pool_id SET NOT NULL;
ALTER TABLE dni_assignments ADD CONSTRAINT dni_assignments_pool_fkey
    FOREIGN KEY (pool_id) REFERENCES dni_pools(id);
ALTER TABLE dni_assignments DROP CONSTRAINT dni_assignments_site_id_session_id_key;
ALTER TABLE dni_assignments ADD CONSTRAINT dni_assignments_session_pool_unique
    UNIQUE (site_id, session_id, pool_id);
