ALTER TABLE event_log RENAME COLUMN classification TO decoded_classification;
ALTER TABLE event_log ADD COLUMN verified_signer BLOB CHECK (length(verified_signer) = 32);
ALTER TABLE event_log ADD COLUMN classification TEXT GENERATED ALWAYS AS (
    CASE
        WHEN verified_signer IS NOT NULL AND author != verified_signer THEN 'forged_author'
        ELSE decoded_classification
    END
) VIRTUAL;
