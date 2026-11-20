CREATE INDEX event_log_outbox ON event_log (board, author, hlc, id);
