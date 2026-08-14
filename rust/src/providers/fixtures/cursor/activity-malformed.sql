-- Readable database with no ai_code_hashes table. The activity query must
-- fail closed instead of looking like zero usage.
CREATE TABLE other (id INTEGER);
