CREATE TABLE slow_table (id integer PRIMARY KEY);
SELECT pg_sleep(3);
