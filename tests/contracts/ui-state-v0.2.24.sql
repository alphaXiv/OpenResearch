CREATE TABLE ui_state (
    id INTEGER PRIMARY KEY CHECK (id = 1),
    onboarding_completed INTEGER NOT NULL DEFAULT 0,
    tour_completed INTEGER NOT NULL DEFAULT 0,
    preferred_harness TEXT,
    preferred_model TEXT,
    preferred_service_tier TEXT,
    preferred_permission_mode TEXT,
    preferred_reasoning_level TEXT,
    workspace_state_json TEXT,
    preferred_autonomy TEXT
);
INSERT INTO ui_state (id) VALUES (1);
