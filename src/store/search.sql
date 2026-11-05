CREATE VIRTUAL TABLE IF NOT EXISTS chat_search USING fts5(
    text, tokenize = 'trigram', content = '', contentless_delete = 1
);

CREATE INDEX IF NOT EXISTS idx_chat_sessions_project ON chat_sessions(project_id);

-- Every writer, old binaries included, runs these triggers, so the view must never fail on stored JSON.
CREATE VIEW IF NOT EXISTS chat_search_parts AS
SELECT m.rowid AS message_rowid,
    json_extract(p.value, '$.id') AS part_id,
    json_extract(p.value, '$.parent') AS spawn_part_id,
    coalesce(json_extract(p.value, '$.text'), '') AS text
FROM chat_messages m, json_each((
    WITH RECURSIVE parts(value, parent) AS (
        SELECT value, NULL FROM json_each(CASE WHEN json_valid(m.parts_json) THEN m.parts_json END)
        WHERE type = 'object'
        UNION ALL
        SELECT child.value, CASE WHEN json_extract(parts.value, '$.type') = 'tool'
            THEN json_extract(parts.value, '$.id') ELSE parts.parent END
        FROM parts, json_each(parts.value, '$.children') child
        WHERE child.type = 'object'
    ), visible AS (
        SELECT value, parent,
            lower(json_extract(value, '$.tool')) IN ('subagent', 'task', 'agent')
                OR json_extract(value, '$.state.title') = 'subagent'
                OR json_array_length(value, '$.children') > 0 AS agent
        FROM parts
    )
    SELECT json_group_array(json_object(
        'id', json_extract(value, '$.id'),
        'parent', CASE WHEN agent THEN json_extract(value, '$.id') ELSE parent END,
        'text', CASE json_extract(value, '$.type')
            WHEN 'text' THEN json_extract(value, '$.text')
            WHEN 'steer' THEN json_extract(value, '$.text')
            WHEN 'tool' THEN concat_ws(char(10),
                json_extract(value, '$.state.input.command'),
                json_extract(value, '$.state.input.cmd'),
                json_extract(value, '$.state.input.file_path'),
                json_extract(value, '$.state.input.filePath'),
                json_extract(value, '$.state.input.path'),
                json_extract(value, '$.state.input.pattern'),
                json_extract(value, '$.state.input.query'),
                CASE WHEN agent THEN json_extract(value, '$.state.input.description') END,
                CASE WHEN json_extract(value, '$.state.status') = 'error'
                    THEN coalesce(nullif(json_extract(value, '$.state.error'), ''), json_extract(value, '$.state.output')) END,
                CASE WHEN m.role = 'user' AND json_extract(value, '$.tool') = 'bash'
                    THEN json_extract(value, '$.state.output') END,
                CASE WHEN agent AND json_extract(value, '$.state.status') = 'completed'
                    AND coalesce(json_extract(value, '$.state.output'), '') NOT LIKE 'Async agent launched%'
                    AND coalesce(json_extract(value, '$.state.output'), '') NOT LIKE 'The subagent is working in the background%'
                    AND NOT EXISTS (SELECT 1 FROM json_each(visible.value, '$.children') child
                        WHERE child.type = 'object'
                            AND json_extract(child.value, '$.type') = 'text'
                            AND length(json_extract(child.value, '$.text')) > 0)
                    THEN json_extract(value, '$.state.output') END)
            WHEN 'prompt' THEN concat_ws(char(10),
                json_extract(value, '$.prompt.question'),
                json_extract(value, '$.prompt.plan'))
        END)) FROM visible
)) p;

CREATE VIEW IF NOT EXISTS chat_search_text AS
SELECT m.rowid AS message_rowid,
    coalesce((SELECT group_concat(text, char(10)) FROM chat_search_parts p WHERE p.message_rowid = m.rowid), '') AS text
FROM chat_messages m;

CREATE TRIGGER IF NOT EXISTS chat_search_insert AFTER INSERT ON chat_messages BEGIN
    INSERT INTO chat_search(rowid, text)
    SELECT message_rowid, text FROM chat_search_text WHERE message_rowid = new.rowid;
END;

CREATE TRIGGER IF NOT EXISTS chat_search_update AFTER UPDATE OF parts_json ON chat_messages
WHEN old.parts_json != new.parts_json BEGIN
    DELETE FROM chat_search WHERE rowid = old.rowid;
    INSERT INTO chat_search(rowid, text)
    SELECT message_rowid, text FROM chat_search_text WHERE message_rowid = new.rowid;
END;

CREATE TRIGGER IF NOT EXISTS chat_search_delete AFTER DELETE ON chat_messages BEGIN
    DELETE FROM chat_search WHERE rowid = old.rowid;
END;

CREATE TABLE IF NOT EXISTS chat_search_backfill (
    id INTEGER PRIMARY KEY CHECK (id = 1),
    through_rowid INTEGER NOT NULL
);

INSERT OR IGNORE INTO chat_search_backfill
SELECT 1, 0 WHERE (SELECT user_version FROM pragma_user_version) < 2;
