CREATE TABLE IF NOT EXISTS skipped_table (
    id      integer CONSTRAINT skippedkey PRIMARY KEY,
    title   varchar(40) NOT NULL
);
