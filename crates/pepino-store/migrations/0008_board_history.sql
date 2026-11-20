CREATE INDEX event_log_history ON event_log (board, hlc, id) WHERE hlc IS NOT NULL;
CREATE INDEX event_log_unordered ON event_log (board, id) WHERE hlc IS NULL;
