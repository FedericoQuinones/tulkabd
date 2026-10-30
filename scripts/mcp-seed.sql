-- Seed for the `tulkabd mcp` smoke test (scripts/mcp_smoke.py). Idempotent.
IF DB_ID(N'tulka_mcp') IS NULL CREATE DATABASE tulka_mcp;
GO
USE tulka_mcp;
GO
IF OBJECT_ID(N'dbo.v_pedidos_cliente', N'V') IS NOT NULL DROP VIEW dbo.v_pedidos_cliente;
IF OBJECT_ID(N'dbo.pedidos', N'U') IS NOT NULL DROP TABLE dbo.pedidos;
IF OBJECT_ID(N'dbo.clientes', N'U') IS NOT NULL DROP TABLE dbo.clientes;
IF OBJECT_ID(N'dbo.seq', N'SO') IS NOT NULL DROP SEQUENCE dbo.seq;
GO
CREATE SEQUENCE dbo.seq START WITH 1 INCREMENT BY 1;
CREATE TABLE dbo.clientes (
    id INT NOT NULL CONSTRAINT PK_clientes PRIMARY KEY,
    nombre NVARCHAR(100) NOT NULL,
    email NVARCHAR(200) NULL
);
CREATE TABLE dbo.pedidos (
    id INT NOT NULL CONSTRAINT PK_pedidos PRIMARY KEY,
    cliente_id INT NOT NULL CONSTRAINT FK_pedidos_clientes REFERENCES dbo.clientes (id),
    total DECIMAL(18, 2) NOT NULL,
    creado DATETIME2 NOT NULL
);
GO
INSERT INTO dbo.clientes (id, nombre, email)
SELECT n, CONCAT(N'Cliente ', n),
       CASE WHEN n = 50 THEN N'' WHEN n % 7 = 0 THEN NULL ELSE CONCAT(N'cliente', n, N'@example.com') END
FROM (SELECT TOP (50) ROW_NUMBER() OVER (ORDER BY (SELECT NULL)) AS n FROM sys.all_objects) t;
INSERT INTO dbo.pedidos (id, cliente_id, total, creado)
SELECT n, ((n - 1) % 50) + 1, CAST((n % 997) + 0.25 AS DECIMAL(18, 2)), DATEADD(MINUTE, n, '2026-01-01')
FROM (SELECT TOP (5000) ROW_NUMBER() OVER (ORDER BY (SELECT NULL)) AS n
      FROM sys.all_objects a CROSS JOIN sys.all_objects b) t;
GO
CREATE INDEX IX_pedidos_creado ON dbo.pedidos (creado);
GO
CREATE VIEW dbo.v_pedidos_cliente AS
SELECT p.id, c.nombre, p.total, p.creado FROM dbo.pedidos p JOIN dbo.clientes c ON c.id = p.cliente_id;
GO
