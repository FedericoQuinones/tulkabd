-- Seeds 150 databases (tulka_seed_001..150) with 500 objects each
-- (400 tables, 50 views, 50 procedures) to measure Explorer indexing and
-- palette search. Idempotent: databases that already have 500 user objects
-- are skipped. Run it with the container from scripts/mssql-test.sh:
--   docker cp scripts/mssql-seed-many.sql tulka-mssql:/tmp/seed.sql
--   docker exec tulka-mssql /opt/mssql-tools18/bin/sqlcmd -C -S localhost -U sa -P 'Tulka_Test_2026!' -b -i /tmp/seed.sql
SET NOCOUNT ON;
DECLARE @i INT = 1, @db SYSNAME, @sql NVARCHAR(MAX);
WHILE @i <= 150
BEGIN
  SET @db = N'tulka_seed_' + RIGHT(N'000' + CAST(@i AS NVARCHAR(3)), 3);
  IF DB_ID(@db) IS NULL
  BEGIN
    SET @sql = N'CREATE DATABASE ' + QUOTENAME(@db);
    EXEC (@sql);
    SET @sql = N'ALTER DATABASE ' + QUOTENAME(@db) + N' SET RECOVERY SIMPLE';
    EXEC (@sql);
  END;
  SET @sql = N'USE ' + QUOTENAME(@db) + N';
    IF (SELECT COUNT(*) FROM sys.objects WHERE is_ms_shipped = 0 AND type IN (''U'',''V'',''P'')) < 500
    BEGIN
      DECLARE @n INT = 1, @s NVARCHAR(MAX) = N'''';
      WHILE @n <= 400
      BEGIN
        IF OBJECT_ID(N''dbo.table_'' + CAST(@n AS NVARCHAR(4))) IS NULL
          SET @s += N''CREATE TABLE dbo.table_'' + CAST(@n AS NVARCHAR(4)) + N'' (id INT PRIMARY KEY, name NVARCHAR(50), created DATETIME2);'' + CHAR(10);
        SET @n += 1;
      END;
      SET @n = 1;
      WHILE @n <= 50
      BEGIN
        IF OBJECT_ID(N''dbo.view_'' + CAST(@n AS NVARCHAR(4))) IS NULL
          SET @s += N''EXEC(N''''CREATE VIEW dbo.view_'' + CAST(@n AS NVARCHAR(4)) + N'' AS SELECT id, name FROM dbo.table_'' + CAST(@n AS NVARCHAR(4)) + N'''''');'' + CHAR(10);
        IF OBJECT_ID(N''dbo.proc_'' + CAST(@n AS NVARCHAR(4))) IS NULL
          SET @s += N''EXEC(N''''CREATE PROCEDURE dbo.proc_'' + CAST(@n AS NVARCHAR(4)) + N'' AS SELECT 1'''');'' + CHAR(10);
        SET @n += 1;
      END;
      EXEC (@s);
    END;';
  EXEC (@sql);
  SET @i += 1;
END;
SELECT COUNT(*) AS seed_databases FROM sys.databases WHERE name LIKE N'tulka_seed_%';
