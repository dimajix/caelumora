-- The kinds whose documents are kept here (issue 03): each in a table of its
-- own, made in code when the kind is first opened, of the form its
-- `schema_version` names.
CREATE TABLE kinds (
    name text PRIMARY KEY,
    table_name text NOT NULL UNIQUE,
    schema_version integer NOT NULL
);
